//! Child process execution with argument arrays (never shell strings),
//! streamed output, timeout, cancellation and whole-tree termination.

use crate::{Diagnostic, Result};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct RunSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(String, OsString)>,
    pub timeout: Option<Duration>,
}

#[derive(Debug)]
pub struct RunOutput {
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub timed_out: bool,
    pub duration: Duration,
    /// Bounded tail of combined stdout/stderr.
    pub tail: String,
    pub command: String,
}

impl RunOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.cancelled && !self.timed_out
    }
}

impl RunSpec {
    /// Human-readable command for logs and diagnostics (not executed).
    pub fn display(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

const TAIL_LINES: usize = 120;
const MAX_LINE: usize = 4000;

/// Run to completion, calling `on_line` for every output line. Cancelling the
/// token or hitting the timeout kills the entire process tree.
pub fn run(spec: &RunSpec, cancel: &CancelToken, mut on_line: impl FnMut(&str)) -> Result<RunOutput> {
    let command = spec.display();
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    tree::prepare(&mut cmd);

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| {
        let mut d = Diagnostic::new(format!("Could not start process: {e}"));
        d.command = Some(command.clone());
        d
    })?;
    let tree = tree::ProcTree::attach(&child);

    let (tx, rx) = mpsc::channel::<String>();
    let readers = [
        child.stdout.take().map(|s| spawn_reader(s, tx.clone())),
        child.stderr.take().map(|s| spawn_reader(s, tx.clone())),
    ];
    drop(tx);

    let mut tail: VecDeque<String> = VecDeque::with_capacity(TAIL_LINES);
    let mut cancelled = false;
    let mut timed_out = false;
    let mut exited_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                on_line(&line);
                tail.push_back(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !cancelled && !timed_out {
            if cancel.is_cancelled() {
                cancelled = true;
            } else if spec.timeout.is_some_and(|t| started.elapsed() > t) {
                timed_out = true;
            }
            if cancelled || timed_out {
                tree.kill();
                let _ = child.kill();
            }
        }
        // The main process may exit while a descendant keeps the pipes open.
        if exited_at.is_none() && matches!(child.try_wait(), Ok(Some(_))) {
            exited_at = Some(Instant::now());
        }
        if exited_at.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            tree.kill();
            break;
        }
    }
    let status = child.wait().ok();
    // Make sure no descendant (browser, ffmpeg, dev server) outlives the job.
    tree.kill();
    drop(rx);
    for r in readers.into_iter().flatten() {
        let _ = r.join();
    }

    Ok(RunOutput {
        exit_code: status.and_then(|s| s.code()),
        cancelled,
        timed_out,
        duration: started.elapsed(),
        tail: tail.into_iter().collect::<Vec<_>>().join("\n"),
        command,
    })
}

fn spawn_reader(mut stream: impl Read + Send + 'static, tx: mpsc::Sender<String>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut line: Vec<u8> = Vec::new();
        let flush = |line: &mut Vec<u8>| {
            if !line.is_empty() {
                let mut s = String::from_utf8_lossy(line).into_owned();
                if s.len() > MAX_LINE {
                    let mut cut = MAX_LINE;
                    while !s.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    s.truncate(cut);
                }
                line.clear();
                return tx.send(s).is_ok();
            }
            true
        };
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        // Progress bars use '\r'; treat it as a line break too.
                        if b == b'\n' || b == b'\r' {
                            if !flush(&mut line) {
                                return;
                            }
                        } else {
                            line.push(b);
                        }
                    }
                }
            }
        }
        flush(&mut line);
    })
}

#[cfg(unix)]
mod tree {
    use super::*;
    use std::os::unix::process::CommandExt;

    pub fn prepare(cmd: &mut Command) {
        // Own process group so the whole tree can be signalled at once.
        cmd.process_group(0);
    }

    pub struct ProcTree {
        pgid: i32,
    }

    impl ProcTree {
        pub fn attach(child: &Child) -> Self {
            ProcTree { pgid: child.id() as i32 }
        }
        pub fn kill(&self) {
            if self.pgid > 1 {
                unsafe {
                    libc::kill(-self.pgid, libc::SIGKILL);
                }
            }
        }
    }
}

#[cfg(windows)]
mod tree {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn prepare(cmd: &mut Command) {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    /// A Job Object holding the child and all its descendants. Closing the
    /// handle (including when this app crashes) terminates every member.
    pub struct ProcTree {
        job: HANDLE,
    }

    unsafe impl Send for ProcTree {}
    unsafe impl Sync for ProcTree {}

    impl ProcTree {
        pub fn attach(child: &Child) -> Self {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if !job.is_null() {
                    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                    SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        &info as *const _ as *const core::ffi::c_void,
                        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                    AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
                }
                ProcTree { job }
            }
        }
        pub fn kill(&self) {
            if !self.job.is_null() {
                unsafe {
                    TerminateJobObject(self.job, 1);
                }
            }
        }
    }

    impl Drop for ProcTree {
        fn drop(&mut self) {
            if !self.job.is_null() {
                unsafe {
                    CloseHandle(self.job);
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str, timeout: Option<Duration>) -> RunSpec {
        RunSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![],
            timeout,
        }
    }

    #[test]
    fn captures_output_and_exit_code() {
        let mut lines = Vec::new();
        let out = run(&sh("echo one; printf 'a\\rb\\n' >&2; exit 3", None), &CancelToken::new(), |l| lines.push(l.to_string())).unwrap();
        assert_eq!(out.exit_code, Some(3));
        assert!(!out.success());
        for expected in ["one", "a", "b"] {
            assert!(lines.iter().any(|l| l == expected), "{lines:?}");
        }
    }

    #[test]
    fn arguments_are_not_shell_interpreted() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("pwned");
        let spec = RunSpec {
            program: "/bin/echo".into(),
            args: vec![format!("; touch {}", marker.display()).into()],
            cwd: tmp.path().into(),
            env: vec![],
            timeout: None,
        };
        assert!(run(&spec, &CancelToken::new(), |_| {}).unwrap().success());
        assert!(!marker.exists());
    }

    #[test]
    fn cancel_kills_descendants() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("pid");
        // The grandchild writes its pid, then sleeps.
        let script = format!("(sh -c 'echo $$ > {}; sleep 60') & sleep 60", pidfile.display());
        let token = CancelToken::new();
        let t2 = token.clone();
        let pf = pidfile.clone();
        let canceller = std::thread::spawn(move || {
            while !pf.exists() || std::fs::read_to_string(&pf).unwrap().trim().is_empty() {
                std::thread::sleep(Duration::from_millis(20));
            }
            t2.cancel();
        });
        let started = Instant::now();
        let out = run(&sh(&script, None), &token, |_| {}).unwrap();
        canceller.join().unwrap();
        assert!(out.cancelled);
        assert!(started.elapsed() < Duration::from_secs(20));
        let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        // A killed process may linger as a zombie until its parent reaps it.
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rsplit(')').next().and_then(|r| r.trim().chars().next()));
        assert!(matches!(state, None | Some('Z') | Some('X')), "grandchild {pid} survived cancellation (state {state:?})");
    }

    #[test]
    fn timeout_is_enforced() {
        let out = run(&sh("sleep 30", Some(Duration::from_millis(300))), &CancelToken::new(), |_| {}).unwrap();
        assert!(out.timed_out);
        assert!(out.duration < Duration::from_secs(10));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn spec(program: &str, args: &[&str]) -> RunSpec {
        RunSpec { program: program.into(), args: args.iter().map(|a| a.into()).collect(), cwd: std::env::temp_dir(), env: vec![], timeout: None }
    }

    #[test]
    fn captures_output_and_exit_code() {
        let mut lines = Vec::new();
        let out = run(&spec("cmd", &["/C", "echo one& exit 3"]), &CancelToken::new(), |l| lines.push(l.to_string())).unwrap();
        assert_eq!(out.exit_code, Some(3));
        assert!(lines.iter().any(|l| l.trim() == "one"), "{lines:?}");
    }

    #[test]
    fn cancel_kills_the_whole_job() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("pid.txt");
        // PowerShell starts a grandchild (ping), records its PID, then waits.
        let script = format!(
            "$p = Start-Process ping -ArgumentList '-n','120','127.0.0.1' -PassThru -WindowStyle Hidden; Set-Content -Path '{}' -Value $p.Id; Start-Sleep 120",
            pidfile.display()
        );
        let token = CancelToken::new();
        let (t2, pf) = (token.clone(), pidfile.clone());
        let canceller = std::thread::spawn(move || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(60) {
                if std::fs::read_to_string(&pf).is_ok_and(|s| !s.trim().is_empty()) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            t2.cancel();
        });
        let started = Instant::now();
        let out = run(&spec("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]), &token, |_| {}).unwrap();
        canceller.join().unwrap();
        assert!(out.cancelled);
        assert!(started.elapsed() < Duration::from_secs(90));
        let pid = std::fs::read_to_string(&pidfile).expect("grandchild pid").trim().to_string();
        std::thread::sleep(Duration::from_millis(500));
        let list = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        let text = String::from_utf8_lossy(&list.stdout);
        assert!(!text.contains("ping"), "grandchild {pid} survived cancellation: {text}");
    }
}
