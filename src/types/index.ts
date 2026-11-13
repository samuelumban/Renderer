// Mirrors src-tauri/core/src/model.rs (camelCase JSON).

export type StorageMode = "fast-reopen" | "save-space";
export type EnvStatus = "not-prepared" | "preparing" | "ready" | "failed" | "removed";
export type JobStatus = "waiting" | "preparing" | "rendering" | "completed" | "failed" | "cancelled";

export interface Diagnostic {
  summary: string;
  command?: string | null;
  exitCode?: number | null;
  excerpt?: string | null;
  likelyCause?: string | null;
  causeIsHypothesis: boolean;
  suggestion?: string | null;
}

export interface PackageVersion {
  name: string;
  declared?: string | null;
  locked?: string | null;
  installed?: string | null;
}

export interface CompositionInfo {
  id: string;
  width?: number | null;
  height?: number | null;
  fps?: number | null;
  durationInFrames?: number | null;
  source: string;
}

export interface FontReport {
  files: string[];
  declaredFamilies: string[];
  referencedFamilies: string[];
  external: string[];
  googleFonts: string[];
  possiblyMissing: string[];
}

export interface Issue {
  severity: "error" | "warning" | "info";
  code: string;
  message: string;
  confirmed: boolean;
}

export interface InspectionReport {
  rootRel: string;
  packageName?: string | null;
  remotionPackages: PackageVersion[];
  reactVersion?: string | null;
  nodeRequirement?: string | null;
  nodeRequirementSource?: string | null;
  packageManager: string;
  packageManagerVersion?: string | null;
  lockfile?: string | null;
  installCommand: string;
  entryPoint?: string | null;
  compositions: CompositionInfo[];
  fonts: FontReport;
  issues: Issue[];
  inspectedAt: string;
}

export interface Project {
  id: string;
  name: string;
  originalZipPath: string;
  zipCopyPath?: string | null;
  workspacePath: string;
  projectRoot: string;
  packageManager: string;
  lockfilePath?: string | null;
  remotionVersion?: string | null;
  nodeRequirement?: string | null;
  selectedNodeRuntime?: string | null;
  nodeVersion?: string | null;
  storageMode: StorageMode;
  envStatus: EnvStatus;
  installApproved: boolean;
  safeInstall: boolean;
  lastError?: Diagnostic | null;
  report: InspectionReport;
  createdAt: string;
  lastOpenedAt?: string | null;
  lastRenderAt?: string | null;
}

export interface RenderRequest {
  projectId: string;
  compositionId: string;
  outputDir: string;
  fileName: string;
  codec: string;
  scale?: number | null;
  concurrency?: number | null;
  frameStart?: number | null;
  frameEnd?: number | null;
  overwrite: boolean;
}

export interface RenderJob {
  id: string;
  projectId: string;
  projectName: string;
  compositionId: string;
  outputPath: string;
  status: JobStatus;
  createdAt: string;
  startedAt?: string | null;
  finishedAt?: string | null;
  exitCode?: number | null;
  logPath: string;
  temporaryPaths: string[];
  request: RenderRequest;
  frame: number;
  totalFrames: number;
  phase: string;
  outputBytes?: number | null;
  error?: Diagnostic | null;
}

export interface Settings {
  outputDir: string;
  defaultStorageMode: StorageMode;
  keepZipCopy: boolean;
  cacheLimitBytes: number;
  maxZipBytes: number;
  maxExtractedBytes: number;
  maxZipEntries: number;
  installTimeoutMinutes: number;
  maxConcurrentRenders: number;
  defaultNodeMajor: number;
  nodeMirror: string;
  browserExecutable?: string | null;
}

export interface StorageUsage {
  workspaces: number;
  runtimes: number;
  packageCache: number;
  browserCache: number;
  temp: number;
  logs: number;
  freeBytes?: number | null;
  cacheLimitBytes: number;
  baseDir: string;
}

export interface PreviewInfo {
  projectId: string;
  port: number;
  url: string;
  status: "starting" | "running" | "stopped" | "failed";
  error?: Diagnostic | null;
}

export interface LogFile {
  name: string;
  bytes: number;
  modified: string;
}

export interface PrepareOptions {
  allowScripts: boolean;
  allowGenerateLockfile: boolean;
  clean: boolean;
}

export type AppEvent =
  | { type: "log"; key: string; line: string }
  | { type: "projects" }
  | { type: "jobs" }
  | { type: "previews" }
  | { type: "progress"; jobId: string; frame: number; total: number; phase: string };

export interface Progress {
  frame: number;
  total: number;
  phase: string;
}

export type ProjectTab = "overview" | "environment" | "fonts" | "preview" | "render" | "logs";

export type Route =
  | { page: "projects" }
  | { page: "import" }
  | { page: "queue" }
  | { page: "storage" }
  | { page: "settings" }
  | { page: "project"; id: string; tab: ProjectTab };
