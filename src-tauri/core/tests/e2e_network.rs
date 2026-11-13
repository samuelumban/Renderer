//! Real end-to-end run: downloads a managed Node.js runtime, installs each
//! project from its lockfile and renders through its own Remotion CLI.
//! Needs network access, so it is ignored by default:
//!
//!   URS_E2E_ZIPS=/path/a.zip,/path/b.zip [URS_E2E_BROWSER=/path/to/chrome] \
//!     cargo test --test e2e_network -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use urs_core::app::PrepareOptions;
use urs_core::model::{EnvStatus, Event, JobStatus, RenderRequest};
use urs_core::paths::Paths;
use urs_core::Core;

fn wait<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
#[ignore]
fn real_projects_with_different_remotion_versions() {
    let zips: Vec<PathBuf> = std::env::var("URS_E2E_ZIPS").expect("set URS_E2E_ZIPS").split(',').map(PathBuf::from).collect();
    let base = std::env::var("URS_E2E_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("urs-e2e"));
    let _ = std::fs::remove_dir_all(&base);
    let out_dir = base.join("renders");
    let core = Core::new(
        Paths::new(base.join("appdata")),
        Arc::new(|e| {
            if let Event::Log { key, line } = e {
                println!("[{}] {line}", &key[..key.len().min(12)]);
            }
        }),
    )
    .unwrap();
    let mut s = core.settings();
    s.browser_executable = std::env::var("URS_E2E_BROWSER").ok();
    core.update_settings(s).unwrap();

    let mut versions = Vec::new();
    for zip in &zips {
        let p = core.import_zip(zip, None).unwrap();
        println!("== imported {} (declared Remotion {:?}, pm {}, install: {})", p.name, p.remotion_version, p.package_manager, p.report.install_command);
        core.prepare(&p.id, PrepareOptions { allow_scripts: true, ..Default::default() }).unwrap();
        let p = wait("prepare", 900, || Some(core.project(&p.id).unwrap()).filter(|p| p.env_status != EnvStatus::Preparing));
        assert_eq!(p.env_status, EnvStatus::Ready, "{:#?}", p.last_error);
        println!("== ready: Remotion {:?} on Node {:?}", p.remotion_version, p.node_version);
        versions.push(p.remotion_version.clone().unwrap());

        let comps = core.detect_compositions(&p.id).unwrap();
        println!("== compositions: {comps:?}");
        assert!(comps.iter().any(|c| c.id == "Main" && c.source == "remotion-cli"));

        // Preview on the same prepared workspace.
        let info = core.start_preview(&p.id).unwrap();
        let running = wait("preview", 180, || core.list_previews().into_iter().find(|i| i.project_id == p.id && i.status != "starting"));
        assert_eq!(running.status, "running", "{:#?}", running.error);
        // Localhost only: reachable on loopback, refused on the LAN address.
        assert!(std::net::TcpStream::connect(("127.0.0.1", info.port)).is_ok());
        let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        if udp.connect("192.0.2.1:9").is_ok() {
            let lan = udp.local_addr().unwrap().ip();
            if !lan.is_loopback() {
                let exposed = std::net::TcpStream::connect_timeout(&(lan, info.port).into(), Duration::from_secs(2)).is_ok();
                println!("== preview reachable on {lan}: {exposed}");
                assert!(!exposed, "preview must not be exposed to the local network");
            }
        }
        let http = std::process::Command::new(if cfg!(windows) { "curl.exe" } else { "curl" }).args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "--noproxy", "*", &info.url]).output().unwrap();
        println!("== preview HTTP status: {}", String::from_utf8_lossy(&http.stdout));
        assert_eq!(String::from_utf8_lossy(&http.stdout), "200");
        core.stop_preview(&p.id);
        wait("preview stop", 30, || core.list_previews().is_empty().then_some(()));
        assert!(std::net::TcpStream::connect(("127.0.0.1", info.port)).is_err(), "preview server still alive");

        let job = core
            .enqueue_render(RenderRequest {
                project_id: p.id.clone(),
                composition_id: "Main".into(),
                output_dir: out_dir.to_string_lossy().into_owned(),
                file_name: format!("{}.mp4", p.name),
                codec: "h264".into(),
                concurrency: Some(2),
                ..Default::default()
            })
            .unwrap();
        let done = wait("render", 900, || core.list_jobs().into_iter().find(|j| j.id == job.id && j.status.is_finished()));
        assert_eq!(done.status, JobStatus::Completed, "{:#?}", done.error);
        let size = std::fs::metadata(&done.output_path).unwrap().len();
        println!("== rendered {} ({} bytes), progress {}/{}", done.output_path, size, done.frame, done.total_frames);
        assert!(size > 1000);
        let temp = Path::new(&p.workspace_path).join("temp");
        let leftovers: Vec<_> = std::fs::read_dir(&temp).into_iter().flatten().flatten().collect();
        assert!(leftovers.is_empty(), "temp not cleaned: {leftovers:?}");
    }
    println!("== Remotion versions used side by side: {versions:?}");
    if versions.len() > 1 {
        assert_ne!(versions[0], versions[1]);
    }
    println!("== storage: {:?}", core.storage_usage());
    core.shutdown();
}
