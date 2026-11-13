//! Thin command layer. Anything that touches the disk or a child process
//! runs on a blocking thread so the UI never freezes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use urs_core::app::PrepareOptions;
use urs_core::model::*;
use urs_core::{Core, Diagnostic};

type Res<T> = Result<T, Diagnostic>;
type Shared<'a> = State<'a, Arc<Core>>;

async fn blocking<T: Send + 'static>(core: &Shared<'_>, f: impl FnOnce(Arc<Core>) -> Res<T> + Send + 'static) -> Res<T> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(core))
        .await
        .map_err(|e| Diagnostic::new(format!("Internal error: {e}")))?
}

// --- projects ---------------------------------------------------------------

#[tauri::command]
async fn list_projects(core: Shared<'_>) -> Res<Vec<Project>> {
    Ok(core.list_projects())
}

#[tauri::command]
async fn import_zip(core: Shared<'_>, path: String, name: Option<String>) -> Res<Project> {
    blocking(&core, move |c| c.import_zip(&PathBuf::from(path), name)).await
}

#[tauri::command]
async fn cancel_import(core: Shared<'_>) -> Res<()> {
    core.cancel_import();
    Ok(())
}

#[tauri::command]
async fn open_project(core: Shared<'_>, id: String) -> Res<Project> {
    blocking(&core, move |c| c.open_project(&id)).await
}

#[tauri::command]
async fn rename_project(core: Shared<'_>, id: String, name: String) -> Res<Project> {
    blocking(&core, move |c| c.rename_project(&id, &name)).await
}

#[tauri::command]
async fn set_storage_mode(core: Shared<'_>, id: String, mode: StorageMode) -> Res<Project> {
    blocking(&core, move |c| c.set_storage_mode(&id, mode)).await
}

#[tauri::command]
async fn remove_project(app: AppHandle, core: Shared<'_>, id: String) -> Res<()> {
    close_preview_window(&app, &id);
    blocking(&core, move |c| c.remove_project(&id)).await
}

#[tauri::command]
async fn reinspect(core: Shared<'_>, id: String) -> Res<Project> {
    blocking(&core, move |c| c.reinspect(&id)).await
}

#[tauri::command]
async fn add_fonts(core: Shared<'_>, id: String, files: Vec<String>) -> Res<Project> {
    blocking(&core, move |c| c.add_fonts(&id, &files.into_iter().map(PathBuf::from).collect::<Vec<_>>())).await
}

#[tauri::command]
async fn project_sizes(core: Shared<'_>) -> Res<HashMap<String, u64>> {
    blocking(&core, |c| Ok(c.project_sizes())).await
}

// --- environment --------------------------------------------------------------

#[tauri::command]
async fn prepare(core: Shared<'_>, id: String, options: PrepareOptions) -> Res<()> {
    blocking(&core, move |c| c.prepare(&id, options)).await
}

#[tauri::command]
async fn cancel_prepare(core: Shared<'_>, id: String) -> Res<()> {
    core.cancel_prepare(&id);
    Ok(())
}

#[tauri::command]
async fn detect_compositions(core: Shared<'_>, id: String) -> Res<Vec<CompositionInfo>> {
    blocking(&core, move |c| c.detect_compositions(&id)).await
}

// --- preview --------------------------------------------------------------------

fn preview_label(id: &str) -> String {
    format!("preview-{}", id.chars().filter(char::is_ascii_alphanumeric).take(16).collect::<String>())
}

fn close_preview_window(app: &AppHandle, id: &str) {
    if let Some(w) = app.get_webview_window(&preview_label(id)) {
        let _ = w.close();
    }
}

fn running_preview(core: &Core, id: &str) -> Res<PreviewInfo> {
    core.list_previews()
        .into_iter()
        .find(|p| p.project_id == id && p.status == "running")
        .ok_or_else(|| Diagnostic::new("The preview is not running."))
}

#[tauri::command]
async fn start_preview(core: Shared<'_>, id: String) -> Res<PreviewInfo> {
    blocking(&core, move |c| c.start_preview(&id)).await
}

#[tauri::command]
async fn stop_preview(app: AppHandle, core: Shared<'_>, id: String) -> Res<()> {
    close_preview_window(&app, &id);
    core.stop_preview(&id);
    Ok(())
}

#[tauri::command]
async fn list_previews(core: Shared<'_>) -> Res<Vec<PreviewInfo>> {
    Ok(core.list_previews())
}

/// Open Remotion Studio in its own app window. That window gets no access to
/// app commands: capabilities are granted to the "main" window only.
#[tauri::command]
async fn open_preview_window(app: AppHandle, core: Shared<'_>, id: String) -> Res<()> {
    let info = running_preview(&core, &id)?;
    let label = preview_label(&id);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.set_focus();
        return Ok(());
    }
    let url = info.url.parse().map_err(|_| Diagnostic::new("Invalid preview URL."))?;
    let name = core.project(&id).map(|p| p.name).unwrap_or_default();
    WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url))
        .title(format!("Preview - {name}"))
        .inner_size(1280.0, 800.0)
        .build()
        .map(|_| ())
        .map_err(|e| Diagnostic::new(format!("Could not open the preview window: {e}")))
}

#[tauri::command]
async fn open_preview_in_browser(app: AppHandle, core: Shared<'_>, id: String) -> Res<()> {
    let info = running_preview(&core, &id)?;
    app.opener()
        .open_url(info.url, None::<&str>)
        .map_err(|e| Diagnostic::new(format!("Could not open the browser: {e}")))
}

// --- render ---------------------------------------------------------------------

#[tauri::command]
async fn enqueue_render(core: Shared<'_>, request: RenderRequest) -> Res<RenderJob> {
    blocking(&core, move |c| c.enqueue_render(request)).await
}

#[tauri::command]
async fn cancel_job(core: Shared<'_>, id: String) -> Res<()> {
    blocking(&core, move |c| {
        c.cancel_job(&id);
        Ok(())
    })
    .await
}

#[tauri::command]
async fn list_jobs(core: Shared<'_>) -> Res<Vec<RenderJob>> {
    Ok(core.list_jobs())
}

#[tauri::command]
async fn clear_finished_jobs(core: Shared<'_>) -> Res<()> {
    core.clear_finished_jobs();
    Ok(())
}

/// Show a finished render in Explorer. Only paths of known jobs are accepted.
#[tauri::command]
async fn reveal_job_output(app: AppHandle, core: Shared<'_>, id: String) -> Res<()> {
    let job = core.list_jobs().into_iter().find(|j| j.id == id).ok_or_else(|| Diagnostic::new("Job not found."))?;
    app.opener()
        .reveal_item_in_dir(&job.output_path)
        .map_err(|e| Diagnostic::new(format!("Could not open the folder: {e}")))
}

#[tauri::command]
async fn open_data_folder(app: AppHandle, core: Shared<'_>) -> Res<()> {
    app.opener()
        .open_path(core.paths.base.to_string_lossy(), None::<&str>)
        .map_err(|e| Diagnostic::new(format!("Could not open the folder: {e}")))
}

// --- storage, logs, settings ------------------------------------------------------

#[tauri::command]
async fn storage_usage(core: Shared<'_>) -> Res<StorageUsage> {
    blocking(&core, |c| Ok(c.storage_usage())).await
}

#[tauri::command]
async fn clean_temp(core: Shared<'_>) -> Res<u64> {
    blocking(&core, |c| c.clean_temp()).await
}

#[tauri::command]
async fn clean_now(core: Shared<'_>) -> Res<u64> {
    blocking(&core, |c| c.clean_now()).await
}

#[tauri::command]
async fn clear_package_cache(core: Shared<'_>) -> Res<u64> {
    blocking(&core, |c| c.clear_package_cache()).await
}

#[tauri::command]
async fn remove_unused_runtimes(core: Shared<'_>) -> Res<u64> {
    blocking(&core, |c| c.remove_unused_runtimes()).await
}

#[tauri::command]
async fn delete_logs(core: Shared<'_>, id: Option<String>) -> Res<u64> {
    blocking(&core, move |c| c.delete_logs(id.as_deref())).await
}

#[tauri::command]
async fn list_logs(core: Shared<'_>, id: String) -> Res<Vec<LogFile>> {
    blocking(&core, move |c| Ok(c.list_logs(&id))).await
}

#[tauri::command]
async fn read_log(core: Shared<'_>, id: String, name: String) -> Res<String> {
    blocking(&core, move |c| c.read_log(&id, &name)).await
}

#[tauri::command]
async fn get_settings(core: Shared<'_>) -> Res<Settings> {
    Ok(core.settings())
}

#[tauri::command]
async fn update_settings(core: Shared<'_>, settings: Settings) -> Res<Settings> {
    blocking(&core, move |c| c.update_settings(settings)).await
}

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        list_projects,
        import_zip,
        cancel_import,
        open_project,
        rename_project,
        set_storage_mode,
        remove_project,
        reinspect,
        add_fonts,
        project_sizes,
        prepare,
        cancel_prepare,
        detect_compositions,
        start_preview,
        stop_preview,
        list_previews,
        open_preview_window,
        open_preview_in_browser,
        enqueue_render,
        cancel_job,
        list_jobs,
        clear_finished_jobs,
        reveal_job_output,
        open_data_folder,
        storage_usage,
        clean_temp,
        clean_now,
        clear_package_cache,
        remove_unused_runtimes,
        delete_logs,
        list_logs,
        read_log,
        get_settings,
        update_settings,
    ]
}
