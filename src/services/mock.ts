// Sample data for browser-only UI development (`npm run dev` without Tauri).
// Never loaded inside the desktop app.
import type { AppEvent, Diagnostic, Project, RenderJob, Settings, StorageUsage } from "../types";

const now = new Date().toISOString();
const GB = 1024 ** 3;

const settings: Settings = {
  outputDir: "C:\\Users\\you\\Videos\\Remotion Renders",
  defaultStorageMode: "fast-reopen",
  keepZipCopy: true,
  cacheLimitBytes: 5 * GB,
  maxZipBytes: GB,
  maxExtractedBytes: 4 * GB,
  maxZipEntries: 200000,
  installTimeoutMinutes: 30,
  maxConcurrentRenders: 1,
  defaultNodeMajor: 22,
  nodeMirror: "https://nodejs.org/dist",
  browserExecutable: null,
};

function project(id: string, name: string, version: string, over: Partial<Project> = {}): Project {
  return {
    id,
    name,
    originalZipPath: `C:\\Users\\you\\Downloads\\${name}.zip`,
    workspacePath: `C:\\Users\\you\\AppData\\Local\\UniversalRemotionStudio\\workspaces\\${id}`,
    projectRoot: `C:\\Users\\you\\AppData\\Local\\UniversalRemotionStudio\\workspaces\\${id}\\source\\${name}`,
    packageManager: "npm",
    remotionVersion: version,
    nodeRequirement: ">=18",
    nodeVersion: "22.11.0",
    selectedNodeRuntime: "C:\\Users\\you\\AppData\\Local\\UniversalRemotionStudio\\runtimes\\node-v22.11.0\\node.exe",
    storageMode: "fast-reopen",
    envStatus: "ready",
    installApproved: true,
    safeInstall: false,
    createdAt: now,
    lastOpenedAt: now,
    lastRenderAt: now,
    report: {
      rootRel: name,
      packageName: name,
      remotionPackages: [
        { name: "@remotion/cli", declared: version, locked: version, installed: version },
        { name: "remotion", declared: version, locked: version, installed: version },
      ],
      reactVersion: "18.2.0",
      nodeRequirement: ">=18",
      nodeRequirementSource: "package.json engines.node",
      packageManager: "npm",
      lockfile: "package-lock.json",
      installCommand: "npm ci",
      entryPoint: "src/index.ts",
      compositions: [
        { id: "Main", width: 1920, height: 1080, fps: 30, durationInFrames: 450, source: "remotion-cli" },
        { id: "Teaser", width: 1080, height: 1920, fps: 30, durationInFrames: 150, source: "remotion-cli" },
      ],
      fonts: {
        files: ["public/fonts/Brand-Bold.woff2"],
        declaredFamilies: ["Brand"],
        referencedFamilies: ["Brand", "Mystery Sans"],
        external: ["https://fonts.googleapis.com/css2?family=Lato"],
        googleFonts: ["Inter"],
        possiblyMissing: ["Mystery Sans"],
      },
      issues: [
        { severity: "warning", code: "fonts-possibly-missing", message: "Font families referenced but not found in the project: Mystery Sans. Chromium may substitute them silently.", confirmed: false },
        { severity: "info", code: "fonts-external", message: "The project loads fonts from the network; rendering needs internet access for them.", confirmed: false },
      ],
      inspectedAt: now,
    },
    ...over,
  };
}

const failure: Diagnostic = {
  summary: "Dependency installation failed (exit code 1).",
  command: "npm ci",
  exitCode: 1,
  excerpt: "npm error code EUSAGE\nnpm error `npm ci` can only install packages when your package.json and package-lock.json are in sync.",
  likelyCause: "The lockfile does not match package.json.",
  causeIsHypothesis: false,
  suggestion: "Regenerate the lockfile in the source project and re-export the ZIP. This app never rewrites a lockfile on its own.",
};

const projects: Project[] = [
  project("a1", "product-launch", "4.0.290"),
  project("b2", "legacy-explainer", "3.3.102", { envStatus: "failed", lastError: failure, lastRenderAt: null, nodeVersion: "18.20.4" }),
  project("c3", "weekly-recap", "4.0.150", { envStatus: "removed", storageMode: "save-space", packageManager: "pnpm" }),
];

function job(id: string, p: Project, over: Partial<RenderJob>): RenderJob {
  return {
    id,
    projectId: p.id,
    projectName: p.name,
    compositionId: "Main",
    outputPath: `${settings.outputDir}\\${p.name}.mp4`,
    status: "waiting",
    createdAt: now,
    logPath: "",
    temporaryPaths: [],
    request: { projectId: p.id, compositionId: "Main", outputDir: settings.outputDir, fileName: `${p.name}.mp4`, codec: "h264", overwrite: false },
    frame: 0,
    totalFrames: 0,
    phase: "waiting",
    ...over,
  };
}

const jobs: RenderJob[] = [
  job("j3", projects[0], { status: "rendering", startedAt: new Date(Date.now() - 42000).toISOString(), frame: 212, totalFrames: 450, phase: "rendering" }),
  job("j2", projects[2], { status: "waiting" }),
  job("j1", projects[0], { status: "completed", startedAt: now, finishedAt: now, frame: 450, totalFrames: 450, outputBytes: 48_300_000, phase: "completed" }),
  job("j0", projects[1], { status: "failed", startedAt: now, finishedAt: now, error: { ...failure, summary: "Render failed (exit code 1).", command: "node remotion-cli.js render src/index.ts Main out.mp4" } }),
];

const usage: StorageUsage = {
  workspaces: 2.4 * GB,
  runtimes: 180 * 1024 ** 2,
  packageCache: 1.1 * GB,
  browserCache: 320 * 1024 ** 2,
  temp: 45 * 1024 ** 2,
  logs: 3 * 1024 ** 2,
  freeBytes: 212 * GB,
  cacheLimitBytes: 5 * GB,
  baseDir: "C:\\Users\\you\\AppData\\Local\\UniversalRemotionStudio",
};

const responses: Record<string, () => unknown> = {
  list_projects: () => projects,
  list_jobs: () => jobs,
  list_previews: () => [{ projectId: "a1", port: 3123, url: "http://localhost:3123", status: "running" }],
  get_settings: () => settings,
  storage_usage: () => usage,
  project_sizes: () => ({ a1: 0.9 * GB, b2: 1.2 * GB, c3: 12 * 1024 ** 2 }),
  list_logs: () => [
    { name: "environment.log", bytes: 18200, modified: now },
    { name: "job-j1.log", bytes: 6400, modified: now },
  ],
  read_log: () => "$ node npm-cli.js ci\nadded 412 packages in 38s\nExit code: Some(0) after 38.2s\nEnvironment ready.",
};

export async function call<T>(cmd: string): Promise<T> {
  const r = responses[cmd];
  if (r) return r() as T;
  if (cmd.startsWith("clean") || cmd.startsWith("clear_package") || cmd.startsWith("remove_unused") || cmd === "delete_logs") return 0 as T;
  return undefined as T;
}

export async function subscribe(handler: (e: AppEvent) => void): Promise<() => void> {
  const lines = ["$ node remotion-cli.js render src/index.ts Main ...", "Bundling 100%", "Rendered 180/450, time remaining: 51s", "Rendered 212/450, time remaining: 44s"];
  const t = setTimeout(() => lines.forEach((line) => handler({ type: "log", key: "job:j3", line })), 50);
  return () => clearTimeout(t);
}
