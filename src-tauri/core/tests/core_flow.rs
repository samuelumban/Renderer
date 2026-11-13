//! Offline end-to-end tests of the orchestration layer. A fake "Node runtime"
//! and fake package manager / Remotion CLI (shell scripts) stand in for the
//! real tools so the flow, cancellation and cleanup rules can be verified
//! without network access.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use urs_core::app::PrepareOptions;
use urs_core::model::{EnvStatus, JobStatus, RenderJob, RenderRequest, StorageMode};
use urs_core::paths::Paths;
use urs_core::Core;

const PKG: &str = r#"{"name":"demo-video","scripts":{"dev":"remotion studio src/index.ts"},
  "dependencies":{"remotion":"4.0.99","@remotion/cli":"4.0.99","react":"18.2.0"}}"#;
const LOCK: &str = r#"{"lockfileVersion":3,"packages":{"node_modules/remotion":{"version":"4.0.99"},"node_modules/@remotion/cli":{"version":"4.0.99"}}}"#;

/// Fake npm: "installs" a fake @remotion/cli whose script is a shell script.
const FAKE_NPM: &str = r#"
[ "$1" = ci ] || { echo "expected npm ci, got $1" >&2; exit 9; }
mkdir -p node_modules/@remotion/cli node_modules/remotion
echo '{"version":"4.0.99","bin":{"remotion":"remotion-cli.js"}}' > node_modules/@remotion/cli/package.json
echo '{"version":"4.0.99"}' > node_modules/remotion/package.json
cat > node_modules/@remotion/cli/remotion-cli.js <<'CLI'
# args: render <entry> <composition> <output> [flags]
[ "$1" = render ] || exit 2
echo "junk" > "$TMPDIR/intermediate-frame.tmp"
echo "Rendered 5/10"
case "$3" in
  Slow) sleep 60 & sleep 60 ;;
  Fail) echo "Error: Could not find composition with ID Fail" >&2; exit 1 ;;
  Empty) : > "$4"; exit 0 ;;
esac
printf 'video-bytes' > "$4"
echo "Encoded 10/10"
CLI
echo "added 2 packages"
"#;

struct Env {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    out: PathBuf,
    zip: PathBuf,
}

fn write_exec(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn make_zip(path: &Path, entries: &[(&str, &str)]) {
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, data) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(data.as_bytes()).unwrap();
    }
    w.finish().unwrap();
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("appdata");
    // Fake managed runtime: "node <script> args..." runs the script with sh.
    let rt = base.join("runtimes/node-v22.0.0");
    write_exec(&rt.join("bin/node"), "#!/bin/sh\nexec /bin/sh \"$@\"\n");
    write_exec(&rt.join("lib/node_modules/npm/bin/npm-cli.js"), FAKE_NPM);
    let zip = tmp.path().join("user").join("demo.zip");
    fs::create_dir_all(zip.parent().unwrap()).unwrap();
    make_zip(
        &zip,
        &[
            ("demo/package.json", PKG),
            ("demo/package-lock.json", LOCK),
            ("demo/src/index.ts", "registerRoot(Root)"),
            ("demo/src/Root.tsx", r#"<Composition id="Main" width={1280} height={720} fps={30} durationInFrames={10} />"#),
        ],
    );
    let out = tmp.path().join("user").join("renders");
    Env { base, out, zip, _tmp: tmp }
}

fn core(env: &Env) -> Arc<Core> {
    Core::new(Paths::new(&env.base), Arc::new(|_| {})).unwrap()
}

fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn prepare(core: &Arc<Core>, id: &str) {
    core.prepare(id, PrepareOptions { allow_scripts: true, ..Default::default() }).unwrap();
    let p = wait("prepare", || Some(core.project(id).unwrap()).filter(|p| p.env_status != EnvStatus::Preparing));
    assert_eq!(p.env_status, EnvStatus::Ready, "{:?}", p.last_error);
}

fn request(env: &Env, id: &str, comp: &str) -> RenderRequest {
    RenderRequest {
        project_id: id.into(),
        composition_id: comp.into(),
        output_dir: env.out.to_string_lossy().into_owned(),
        file_name: "out.mp4".into(),
        codec: "h264".into(),
        ..Default::default()
    }
}

fn finish(core: &Arc<Core>, job: &RenderJob) -> RenderJob {
    wait("job", || core.list_jobs().into_iter().find(|j| j.id == job.id && j.status.is_finished()))
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn import_prepare_render_and_cleanup() {
    let env = setup();
    let core = core(&env);
    let p = core.import_zip(&env.zip, None).unwrap();
    assert_eq!(p.name, "demo-video");
    assert!(p.project_root.ends_with("source/demo"));
    assert_eq!(p.remotion_version.as_deref(), Some("4.0.99"));
    assert_eq!(p.package_manager, "npm");
    assert_eq!(p.report.compositions[0].id, "Main");
    assert!(Path::new(p.zip_copy_path.as_ref().unwrap()).is_file());

    // Preview and render refuse to run before the environment is prepared.
    assert!(core.start_preview(&p.id).is_err());

    prepare(&core, &p.id);
    let p = core.project(&p.id).unwrap();
    assert_eq!(p.node_version.as_deref(), Some("22.0.0"));
    assert!(p.selected_node_runtime.unwrap().starts_with(env.base.to_str().unwrap()));
    assert!(fs::read_to_string(env.base.join("logs").join(&p.id).join("environment.log")).unwrap().contains("npm ci"));

    let job = finish(&core, &core.enqueue_render(request(&env, &p.id, "Main")).unwrap());
    assert_eq!(job.status, JobStatus::Completed, "{:?}", job.error);
    assert_eq!(job.exit_code, Some(0));
    assert_eq!((job.frame, job.total_frames), (10, 10));
    assert!(job.temporary_paths.is_empty());
    assert_eq!(fs::read(env.out.join("out.mp4")).unwrap(), b"video-bytes");
    // No partial files, no leftover temp, and an existing render is not overwritten.
    assert_eq!(names(&env.out), ["out.mp4"]);
    assert!(names(&env.base.join("workspaces").join(&p.id).join("temp")).is_empty());
    let job2 = finish(&core, &core.enqueue_render(request(&env, &p.id, "Main")).unwrap());
    assert!(job2.output_path.ends_with("out (2).mp4"));
    assert_eq!(names(&env.out), ["out (2).mp4", "out.mp4"]);
    // Fast Reopen keeps the workspace.
    assert!(Path::new(&p.project_root).join("node_modules").is_dir());
    assert!(env.zip.is_file());

    // Removing the project never touches the user's ZIP or renders.
    core.remove_project(&p.id).unwrap();
    assert!(!env.base.join("workspaces").join(&p.id).exists());
    assert!(env.zip.is_file());
    assert!(env.out.join("out.mp4").is_file());
}

#[test]
fn cancel_stops_children_and_removes_partial_output() {
    let env = setup();
    let core = core(&env);
    let p = core.import_zip(&env.zip, None).unwrap();
    prepare(&core, &p.id);
    let job = core.enqueue_render(request(&env, &p.id, "Slow")).unwrap();
    let queued = core.enqueue_render(request(&env, &p.id, "Main")).unwrap();
    wait("rendering", || core.list_jobs().into_iter().find(|j| j.id == job.id && j.status == JobStatus::Rendering && j.frame == 0 && !j.temporary_paths.is_empty()));
    // One render at a time by default.
    assert_eq!(core.list_jobs().into_iter().find(|j| j.id == queued.id).unwrap().status, JobStatus::Waiting);
    assert!(core.clear_package_cache().is_err(), "cache must not be cleared during a render");
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    core.cancel_job(&job.id);
    let done = finish(&core, &job);
    assert_eq!(done.status, JobStatus::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(10), "cancel must not wait for the child to finish");
    // The queued job then runs and completes.
    assert_eq!(finish(&core, &queued).status, JobStatus::Completed);
    assert_eq!(names(&env.out), ["out.mp4"]);
    assert!(names(&env.base.join("workspaces").join(&p.id).join("temp")).is_empty());
}

#[test]
fn failed_and_empty_renders_leave_no_output() {
    let env = setup();
    let core = core(&env);
    let p = core.import_zip(&env.zip, None).unwrap();
    prepare(&core, &p.id);
    let failed = finish(&core, &core.enqueue_render(request(&env, &p.id, "Fail")).unwrap());
    assert_eq!(failed.status, JobStatus::Failed);
    let d = failed.error.unwrap();
    assert_eq!(d.exit_code, Some(1));
    assert!(!d.cause_is_hypothesis);
    assert!(d.excerpt.unwrap().contains("Could not find composition"));
    assert!(d.command.unwrap().contains("render"));

    let empty = finish(&core, &core.enqueue_render(request(&env, &p.id, "Empty")).unwrap());
    assert_eq!(empty.status, JobStatus::Failed);
    assert!(empty.error.unwrap().summary.contains("missing or empty"));
    assert!(names(&env.out).is_empty(), "{:?}", names(&env.out));

    assert!(core.enqueue_render(request(&env, &p.id, "--evil")).is_err());
}

#[test]
fn save_space_removes_workspace_but_keeps_zip_and_output() {
    let env = setup();
    let core = core(&env);
    let p = core.import_zip(&env.zip, None).unwrap();
    core.set_storage_mode(&p.id, StorageMode::SaveSpace).unwrap();
    prepare(&core, &p.id);
    let job = finish(&core, &core.enqueue_render(request(&env, &p.id, "Main")).unwrap());
    assert_eq!(job.status, JobStatus::Completed);
    let after = wait("save space", || Some(core.project(&p.id).unwrap()).filter(|p| p.env_status == EnvStatus::Removed));
    assert!(!Path::new(&after.project_root).exists());
    assert!(!env.base.join("workspaces").join(&p.id).join("source").exists());
    assert!(Path::new(after.zip_copy_path.as_ref().unwrap()).is_file());
    assert!(env.zip.is_file());
    assert!(env.out.join("out.mp4").is_file());
    assert!(!after.report.remotion_packages.is_empty(), "metadata is retained");

    // Reopening restores from the ZIP and reinstalls.
    prepare(&core, &p.id);
    assert!(Path::new(&core.project(&p.id).unwrap().project_root).join("node_modules").is_dir());
}

#[test]
fn save_space_never_deletes_the_only_copy() {
    let env = setup();
    let core = core(&env);
    let mut s = core.settings();
    s.keep_zip_copy = false;
    s.default_storage_mode = StorageMode::SaveSpace;
    core.update_settings(s).unwrap();
    let p = core.import_zip(&env.zip, None).unwrap();
    assert!(p.zip_copy_path.is_none());
    prepare(&core, &p.id);
    fs::remove_file(&env.zip).unwrap(); // the user deleted their ZIP
    let job = finish(&core, &core.enqueue_render(request(&env, &p.id, "Main")).unwrap());
    assert_eq!(job.status, JobStatus::Completed);
    let after = wait("skip notice", || Some(core.project(&p.id).unwrap()).filter(|p| p.last_error.is_some()));
    assert_eq!(after.env_status, EnvStatus::Ready);
    assert!(Path::new(&after.project_root).join("package.json").is_file());
}

#[test]
fn unsafe_zip_is_rejected_without_leftovers() {
    let env = setup();
    let core = core(&env);
    let evil = env.zip.with_file_name("evil.zip");
    make_zip(&evil, &[("demo/package.json", PKG), ("../../escape.txt", "x")]);
    let err = core.import_zip(&evil, None).unwrap_err();
    assert!(err.summary.contains("Unsafe entry"));
    assert!(names(&env.base.join("workspaces")).is_empty());
    assert!(core.list_projects().is_empty());
    assert!(!env.base.join("escape.txt").exists());
    assert!(!env.base.parent().unwrap().join("escape.txt").exists());

    let not_project = env.zip.with_file_name("notes.zip");
    make_zip(&not_project, &[("readme.txt", "hello")]);
    assert!(core.import_zip(&not_project, None).is_err());
    assert!(names(&env.base.join("workspaces")).is_empty());
    assert!(core.import_zip(&env.zip.with_file_name("demo.rar"), None).is_err());
}

#[test]
fn startup_recovers_stale_files_after_a_crash() {
    let env = setup();
    let p = {
        let core = core(&env);
        let p = core.import_zip(&env.zip, None).unwrap();
        prepare(&core, &p.id);
        p
    };
    // Simulate a crash mid-render: stale temp, orphan import, partial download,
    // a job stuck in "rendering" with a registered partial output.
    let ws = env.base.join("workspaces");
    fs::create_dir_all(ws.join(&p.id).join("temp/job-x")).unwrap();
    fs::write(ws.join(&p.id).join("temp/job-x/frame.png"), "x").unwrap();
    fs::create_dir_all(ws.join("orphan-import/source")).unwrap();
    fs::write(env.base.join("runtimes/node-v20.0.0-linux-x64.tar.gz.partial"), "x").unwrap();
    fs::create_dir_all(&env.out).unwrap();
    let partial = env.out.join("out.partial-deadbeef.mp4");
    let finished = env.out.join("finished.mp4");
    fs::write(&partial, "half").unwrap();
    fs::write(&finished, "done").unwrap();
    let jobs = serde_json::json!([{
        "id": "deadbeefdeadbeef", "projectId": p.id, "status": "rendering",
        // The second path lacks the app's marker and must never be deleted.
        "temporaryPaths": [partial, finished, ws.join(&p.id).join("temp/job-x")]
    }]);
    fs::write(env.base.join("settings/jobs.json"), jobs.to_string()).unwrap();

    let core = core(&env);
    assert!(!ws.join(&p.id).join("temp").exists());
    assert!(!ws.join("orphan-import").exists());
    assert!(!env.base.join("runtimes/node-v20.0.0-linux-x64.tar.gz.partial").exists());
    assert!(!partial.exists());
    assert!(finished.is_file(), "a finished render must survive recovery");
    assert!(env.zip.is_file());
    let job = &core.list_jobs()[0];
    assert_eq!(job.status, JobStatus::Failed);
    assert!(job.temporary_paths.is_empty());
    let p2 = core.project(&p.id).unwrap();
    assert_eq!(p2.env_status, EnvStatus::Ready);
    assert!(Path::new(&p2.project_root).join("node_modules").is_dir());
}

#[test]
fn storage_usage_and_cache_cleanup() {
    let env = setup();
    let core = core(&env);
    let p = core.import_zip(&env.zip, None).unwrap();
    let cacache = env.base.join("cache/package-manager/npm/_cacache/content-v2");
    fs::create_dir_all(&cacache).unwrap();
    for (i, name) in ["old", "mid", "new"].iter().enumerate() {
        let f = fs::File::create(cacache.join(name)).unwrap();
        f.set_len(50 * 1024 * 1024).unwrap(); // sparse
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(1000 - i as u64 * 100)).unwrap();
    }
    fs::create_dir_all(env.base.join("workspaces").join(&p.id).join("temp/x")).unwrap();
    fs::write(env.base.join("workspaces").join(&p.id).join("temp/x/f"), vec![0u8; 4096]).unwrap();

    let u = core.storage_usage();
    assert_eq!(u.package_cache, 150 * 1024 * 1024);
    assert_eq!(u.temp, 4096);
    assert!(u.workspaces > 0 && u.runtimes > 0);

    let mut s = core.settings();
    s.cache_limit_bytes = 100 * 1024 * 1024;
    core.update_settings(s).unwrap();
    let freed = core.clean_now().unwrap();
    assert_eq!(freed, 4096 + 100 * 1024 * 1024);
    assert_eq!(names(&cacache), ["new"], "least recently written entries go first");

    assert_eq!(core.clear_package_cache().unwrap(), 50 * 1024 * 1024);
    assert!(env.base.join("cache/package-manager").is_dir());

    // The runtime is unused until a project is prepared with it.
    assert!(core.remove_unused_runtimes().unwrap() > 0);
    assert!(!env.base.join("runtimes/node-v22.0.0").exists());
    assert!(Path::new(&p.project_root).join("package.json").is_file());
}

#[test]
fn runtime_in_use_is_kept() {
    let env = setup();
    let core = core(&env);
    let p = core.import_zip(&env.zip, None).unwrap();
    prepare(&core, &p.id);
    core.remove_unused_runtimes().unwrap();
    assert!(env.base.join("runtimes/node-v22.0.0/bin/node").is_file());
}
