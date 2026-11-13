// The only module that talks to the Rust backend.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppEvent, CompositionInfo, Diagnostic, LogFile, PrepareOptions, PreviewInfo, Project,
  RenderJob, RenderRequest, Settings, StorageMode, StorageUsage,
} from "../types";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Args = Record<string, unknown>;
let call: <T>(cmd: string, args?: Args) => Promise<T> = invoke;
let subscribe = (handler: (e: AppEvent) => void): Promise<() => void> =>
  listen<AppEvent>("urs-event", (e) => handler(e.payload));

/** Browser-only development preview (no backend): swap in sample data. */
export async function useMockBackend() {
  const mock = await import("./mock");
  call = mock.call;
  subscribe = mock.subscribe;
}

export function toDiagnostic(e: unknown): Diagnostic {
  if (e && typeof e === "object" && "summary" in e) return e as Diagnostic;
  return { summary: String(e), causeIsHypothesis: false };
}

export const api = {
  onEvent: (handler: (e: AppEvent) => void) => subscribe(handler),

  listProjects: () => call<Project[]>("list_projects"),
  importZip: (path: string, name?: string) => call<Project>("import_zip", { path, name }),
  cancelImport: () => call<void>("cancel_import"),
  openProject: (id: string) => call<Project>("open_project", { id }),
  renameProject: (id: string, name: string) => call<Project>("rename_project", { id, name }),
  setStorageMode: (id: string, mode: StorageMode) => call<Project>("set_storage_mode", { id, mode }),
  removeProject: (id: string) => call<void>("remove_project", { id }),
  reinspect: (id: string) => call<Project>("reinspect", { id }),
  addFonts: (id: string, files: string[]) => call<Project>("add_fonts", { id, files }),
  projectSizes: () => call<Record<string, number>>("project_sizes"),

  prepare: (id: string, options: PrepareOptions) => call<void>("prepare", { id, options }),
  cancelPrepare: (id: string) => call<void>("cancel_prepare", { id }),
  detectCompositions: (id: string) => call<CompositionInfo[]>("detect_compositions", { id }),

  startPreview: (id: string) => call<PreviewInfo>("start_preview", { id }),
  stopPreview: (id: string) => call<void>("stop_preview", { id }),
  listPreviews: () => call<PreviewInfo[]>("list_previews"),
  openPreviewWindow: (id: string) => call<void>("open_preview_window", { id }),
  openPreviewInBrowser: (id: string) => call<void>("open_preview_in_browser", { id }),

  enqueueRender: (request: RenderRequest) => call<RenderJob>("enqueue_render", { request }),
  cancelJob: (id: string) => call<void>("cancel_job", { id }),
  listJobs: () => call<RenderJob[]>("list_jobs"),
  clearFinishedJobs: () => call<void>("clear_finished_jobs"),
  revealJobOutput: (id: string) => call<void>("reveal_job_output", { id }),
  openDataFolder: () => call<void>("open_data_folder"),

  storageUsage: () => call<StorageUsage>("storage_usage"),
  cleanTemp: () => call<number>("clean_temp"),
  cleanNow: () => call<number>("clean_now"),
  clearPackageCache: () => call<number>("clear_package_cache"),
  removeUnusedRuntimes: () => call<number>("remove_unused_runtimes"),
  deleteLogs: (id?: string) => call<number>("delete_logs", { id }),
  listLogs: (id: string) => call<LogFile[]>("list_logs", { id }),
  readLog: (id: string, name: string) => call<string>("read_log", { id, name }),

  getSettings: () => call<Settings>("get_settings"),
  updateSettings: (settings: Settings) => call<Settings>("update_settings", { settings }),
};
