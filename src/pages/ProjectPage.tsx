import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Badge, DiagnosticBox, EnvBadge, Field, JobBadge, LogView, Modal, ProgressBar } from "../components/common";
import type { AppData } from "../hooks/useAppData";
import { api, toDiagnostic } from "../services/api";
import { bytes, compositionLabel, when } from "../services/format";
import type { Diagnostic, Issue, LogFile, PrepareOptions, Project, ProjectTab, Route, StorageMode } from "../types";

const TABS: [ProjectTab, string][] = [
  ["overview", "Overview"],
  ["environment", "Environment"],
  ["fonts", "Fonts"],
  ["preview", "Preview"],
  ["render", "Render"],
  ["logs", "Logs"],
];

interface Props {
  project: Project;
  tab: ProjectTab;
  data: AppData;
  go: (r: Route) => void;
}

export default function ProjectPage({ project, tab, data, go }: Props) {
  const [error, setError] = useState<Diagnostic | null>(null);
  const guard = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(toDiagnostic(e));
    }
  };
  const blocking = project.report.issues.some((i) => i.severity === "error");

  return (
    <>
      <header className="page-head">
        <div>
          <button className="link small" onClick={() => go({ page: "projects" })}>&larr; Projects</button>
          <h1>{project.name}</h1>
          <p className="muted">
            Remotion <span className="mono">{project.remotionVersion ?? "?"}</span> · {project.packageManager} ·{" "}
            <EnvBadge status={project.envStatus} />
          </p>
        </div>
      </header>
      <div className="tabs" role="tablist">
        {TABS.map(([id, label]) => (
          <button key={id} role="tab" aria-selected={tab === id} className={`tab${tab === id ? " active" : ""}`} onClick={() => go({ page: "project", id: project.id, tab: id })}>
            {label}
          </button>
        ))}
      </div>
      {error && <DiagnosticBox d={error} onDismiss={() => setError(null)} />}
      {tab === "overview" && <Overview project={project} guard={guard} />}
      {tab === "environment" && <Environment project={project} data={data} guard={guard} blocking={blocking} />}
      {tab === "fonts" && <Fonts project={project} guard={guard} />}
      {tab === "preview" && <Preview project={project} data={data} guard={guard} go={go} />}
      {tab === "render" && <Render project={project} data={data} guard={guard} go={go} />}
      {tab === "logs" && <Logs project={project} guard={guard} />}
    </>
  );
}

type Guard = (f: () => Promise<unknown>) => Promise<void>;

function IssueList({ issues }: { issues: Issue[] }) {
  if (!issues.length) return <p className="muted">No problems found.</p>;
  const tone = { error: "bad", warning: "warn", info: "info" } as const;
  return (
    <ul className="issues">
      {issues.map((i, n) => (
        <li key={n}>
          <Badge tone={tone[i.severity]}>{i.severity === "error" ? "Blocking" : i.severity === "warning" ? "Warning" : "Note"}</Badge>
          <span>{i.message}</span>
        </li>
      ))}
    </ul>
  );
}

function Overview({ project, guard }: { project: Project; guard: Guard }) {
  const r = project.report;
  const confirmed = r.issues.filter((i) => i.confirmed);
  const possible = r.issues.filter((i) => !i.confirmed);
  const setMode = (mode: StorageMode) => guard(() => api.setStorageMode(project.id, mode));
  return (
    <div className="grid-2">
      <section className="card">
        <h2>Inspection report</h2>
        <dl className="kv">
          <dt>Project root</dt>
          <dd className="mono">{r.rootRel || "(ZIP root)"}</dd>
          <dt>Remotion</dt>
          <dd className="mono">{project.remotionVersion ?? "not found"}</dd>
          <dt>React</dt>
          <dd className="mono">{r.reactVersion ?? "-"}</dd>
          <dt>Node.js requirement</dt>
          <dd>
            <span className="mono">{r.nodeRequirement ?? "none stated"}</span>
            {r.nodeRequirementSource && <span className="muted small"> ({r.nodeRequirementSource})</span>}
          </dd>
          <dt>Selected runtime</dt>
          <dd className="mono">{project.nodeVersion ? `Node v${project.nodeVersion} (app-managed)` : "chosen when the environment is prepared"}</dd>
          <dt>Package manager</dt>
          <dd className="mono">{r.packageManager}{r.packageManagerVersion ? ` ${r.packageManagerVersion}` : ""}</dd>
          <dt>Lockfile</dt>
          <dd className="mono">{r.lockfile ?? "none"}</dd>
          <dt>Install command</dt>
          <dd className="mono">{r.installCommand}</dd>
          <dt>Entry point</dt>
          <dd className="mono">{r.entryPoint ?? "not detected"}</dd>
          <dt>Imported</dt>
          <dd>{when(project.createdAt)}</dd>
          <dt>Original ZIP</dt>
          <dd className="mono small">{project.originalZipPath}</dd>
        </dl>
      </section>
      <section className="card">
        <h2>Confirmed findings</h2>
        <IssueList issues={confirmed} />
        <h2 className="mt">Possible issues</h2>
        <p className="muted small">Found by static scanning; not proven. They may or may not affect the render.</p>
        <IssueList issues={possible} />
      </section>
      <section className="card">
        <h2>Remotion packages</h2>
        <table className="table compact">
          <thead>
            <tr><th>Package</th><th>package.json</th><th>Lockfile</th><th>Installed</th></tr>
          </thead>
          <tbody>
            {r.remotionPackages.map((p) => (
              <tr key={p.name}>
                <td className="mono">{p.name}</td>
                <td className="mono">{p.declared ?? "-"}</td>
                <td className="mono">{p.locked ?? "-"}</td>
                <td className="mono">{p.installed ?? "-"}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <h2 className="mt">Compositions</h2>
        {r.compositions.length ? (
          <ul className="plain">
            {r.compositions.map((c) => (
              <li key={c.id} className="mono">{compositionLabel(c)}</li>
            ))}
          </ul>
        ) : (
          <p className="muted">None detected yet.</p>
        )}
        <p className="muted small">
          {r.compositions.some((c) => c.source === "remotion-cli")
            ? "Listed by the project's Remotion CLI."
            : "Best-effort static scan. Use Detect compositions on the Render tab for the exact list."}
        </p>
      </section>
      <section className="card">
        <h2>Storage mode</h2>
        <label className="radio">
          <input type="radio" checked={project.storageMode === "fast-reopen"} onChange={() => setMode("fast-reopen")} />
          <span>
            <strong>Fast Reopen</strong>
            <span className="muted"> — keep the extracted source and node_modules so the project reopens instantly.</span>
          </span>
        </label>
        <label className="radio">
          <input type="radio" checked={project.storageMode === "save-space"} onChange={() => setMode("save-space")} />
          <span>
            <strong>Save Space</strong>
            <span className="muted"> — after a successful render, delete the workspace and node_modules. The ZIP, report and rendered video are kept; reopening reinstalls dependencies.</span>
          </span>
        </label>
        <p className="muted small">The workspace is only deleted while the ZIP still exists, so the only copy of your source is never removed.</p>
      </section>
    </div>
  );
}

function Environment({ project, data, guard, blocking }: { project: Project; data: AppData; guard: Guard; blocking: boolean }) {
  const [asking, setAsking] = useState<null | { clean: boolean }>(null);
  const [allowScripts, setAllowScripts] = useState(!project.safeInstall);
  const [allowNoLock, setAllowNoLock] = useState(false);
  const noLock = !project.report.lockfile;
  const preparing = project.envStatus === "preparing";
  const key = `env:${project.id}`;

  const start = (clean: boolean) => {
    const options: PrepareOptions = { allowScripts, allowGenerateLockfile: allowNoLock, clean };
    setAsking(null);
    data.clearLog(key);
    void guard(() => api.prepare(project.id, options));
  };

  return (
    <>
      <section className="card">
        <div className="card-head">
          <h2>Environment</h2>
          <EnvBadge status={project.envStatus} />
          <span className="spacer" />
          {preparing ? (
            <button className="btn btn-danger" onClick={() => guard(() => api.cancelPrepare(project.id))}>Cancel</button>
          ) : (
            <>
              {project.envStatus === "ready" ? (
                <button className="btn" disabled={blocking} onClick={() => setAsking({ clean: true })}>Rebuild environment</button>
              ) : (
                <button className="btn btn-primary" disabled={blocking} onClick={() => setAsking({ clean: false })}>Prepare environment</button>
              )}
            </>
          )}
        </div>
        <dl className="kv">
          <dt>Node.js runtime</dt>
          <dd className="mono">{project.nodeVersion ? `v${project.nodeVersion}` : `to be selected (${project.nodeRequirement ?? "default LTS"})`}</dd>
          <dt>Runtime path</dt>
          <dd className="mono small">{project.selectedNodeRuntime ?? "-"}</dd>
          <dt>Install command</dt>
          <dd className="mono">{project.report.installCommand}</dd>
          <dt>Lifecycle scripts</dt>
          <dd>{project.installApproved ? (project.safeInstall ? "Disabled (safe install)" : "Allowed") : "Not approved yet"}</dd>
        </dl>
        {blocking && <p className="warn-text">This project has blocking problems (see Overview) and cannot be prepared.</p>}
        {project.envStatus === "removed" && (
          <p className="muted">The workspace was removed by Save Space mode. Preparing restores it from the ZIP and reinstalls dependencies.</p>
        )}
        <p className="muted small">
          The app uses its own Node.js runtime and the project's lockfile. Your system Node.js, PATH and global packages are never changed, and package versions are never rewritten.
        </p>
      </section>
      {project.lastError && <DiagnosticBox d={project.lastError} />}
      <section className="card">
        <h2>Install log</h2>
        <LogView lines={data.logTail(key)} empty="Output appears here while the environment is prepared. Full logs are in the Logs tab." />
      </section>
      {asking && (
        <Modal title={asking.clean ? "Rebuild environment" : "Prepare environment"} onClose={() => setAsking(null)}>
          <p>
            This will run <code>{project.report.installCommand}</code> for <strong>{project.name}</strong>
            {asking.clean ? " after deleting its node_modules folder." : "."}
          </p>
          <div className="callout">
            <strong>Project code is untrusted.</strong> Installing dependencies can run package lifecycle scripts (preinstall/postinstall) on this computer.
          </div>
          <label className="radio">
            <input type="radio" checked={allowScripts} onChange={() => setAllowScripts(true)} />
            <span><strong>Allow lifecycle scripts</strong><span className="muted"> — needed by some packages. Choose this only if you trust the project.</span></span>
          </label>
          <label className="radio">
            <input type="radio" checked={!allowScripts} onChange={() => setAllowScripts(false)} />
            <span><strong>Safe install</strong><span className="muted"> — scripts disabled. Safer, but some projects may not work in this mode.</span></span>
          </label>
          {noLock && (
            <label className="check">
              <input type="checkbox" checked={allowNoLock} onChange={(e) => setAllowNoLock(e.target.checked)} />
              <span>This project has no lockfile. Install anyway and let the package manager choose versions (not reproducible; a new lockfile is written in the workspace).</span>
            </label>
          )}
          <div className="modal-actions">
            <button className="btn" onClick={() => setAsking(null)}>Cancel</button>
            <button className="btn btn-primary" disabled={noLock && !allowNoLock} onClick={() => start(asking.clean)}>
              {asking.clean ? "Rebuild" : "Install"}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}

function Fonts({ project, guard }: { project: Project; guard: Guard }) {
  const f = project.report.fonts;
  const add = async () => {
    const picked = await open({ multiple: true, filters: [{ name: "Fonts", extensions: ["ttf", "otf", "woff", "woff2"] }] });
    const files = Array.isArray(picked) ? picked : picked ? [picked] : [];
    if (files.length) await guard(() => api.addFonts(project.id, files));
  };
  const list = (items: string[], empty: string) =>
    items.length ? <ul className="plain mono">{items.map((i) => <li key={i}>{i}</li>)}</ul> : <p className="muted">{empty}</p>;
  return (
    <div className="grid-2">
      <section className="card">
        <div className="card-head">
          <h2>Font files in the project</h2>
          <span className="spacer" />
          <button className="btn" onClick={add} disabled={project.envStatus === "removed"}>Add font files...</button>
          <button className="btn" onClick={() => guard(() => api.reinspect(project.id))}>Re-scan</button>
        </div>
        {list(f.files, "No .ttf, .otf, .woff or .woff2 files found.")}
        <p className="muted small">
          Added fonts are copied to <code>public/fonts</code> in the workspace (existing files are never overwritten). The project still has to load them, e.g. with an
          <code> @font-face</code> rule or <code>staticFile("fonts/...")</code>. Fonts are never installed system-wide.
        </p>
      </section>
      <section className="card">
        <h2>Possibly missing fonts</h2>
        {f.possiblyMissing.length ? (
          <>
            <ul className="plain">{f.possiblyMissing.map((i) => <li key={i}><Badge tone="warn">Warning</Badge> {i}</li>)}</ul>
            <p className="muted small">
              These families are referenced but no matching bundled font was found. This is a warning, not a confirmed failure: Chromium substitutes missing fonts silently, so check the preview.
            </p>
          </>
        ) : (
          <p className="muted">No unmatched font families found. A detected family name still does not guarantee the right font file is used.</p>
        )}
      </section>
      <section className="card">
        <h2>Declared families</h2>
        {list(f.declaredFamilies, "No @font-face / FontFace declarations found.")}
        <h2 className="mt">Referenced families</h2>
        {list(f.referencedFamilies, "No font-family references found.")}
      </section>
      <section className="card">
        <h2>Network fonts</h2>
        {list([...f.googleFonts.map((g) => `@remotion/google-fonts/${g}`), ...f.external], "The project does not load fonts from the network.")}
        <p className="muted small">Network fonts are fetched by the project itself at preview/render time and need internet access. The app does not download fonts on its own.</p>
      </section>
    </div>
  );
}

function Preview({ project, data, guard, go }: { project: Project; data: AppData; guard: Guard; go: (r: Route) => void }) {
  const info = data.previews.find((p) => p.projectId === project.id);
  const ready = project.envStatus === "ready";
  const key = `preview:${project.id}`;
  return (
    <>
      <section className="card">
        <div className="card-head">
          <h2>Remotion Studio</h2>
          {info ? (
            <Badge tone={info.status === "running" ? "ok" : info.status === "failed" ? "bad" : "info"}>
              {info.status === "running" ? "Running" : info.status === "failed" ? "Stopped with error" : "Starting..."}
            </Badge>
          ) : (
            <Badge tone="muted">Stopped</Badge>
          )}
          <span className="spacer" />
          {info && info.status !== "failed" ? (
            <button className="btn btn-danger" onClick={() => guard(() => api.stopPreview(project.id))}>Stop preview</button>
          ) : (
            <button
              className="btn btn-primary"
              disabled={!ready}
              onClick={() => {
                data.clearLog(key);
                void guard(() => api.startPreview(project.id));
              }}
            >
              Start preview
            </button>
          )}
        </div>
        {!ready && (
          <p className="warn-text">
            The environment is not ready.{" "}
            <button className="link" onClick={() => go({ page: "project", id: project.id, tab: "environment" })}>Prepare it first</button>.
          </p>
        )}
        {info?.status === "running" && (
          <div className="preview-actions">
            <span className="mono">{info.url}</span>
            <button className="btn btn-primary" onClick={() => guard(() => api.openPreviewWindow(project.id))}>Open in app window</button>
            <button className="btn" onClick={() => guard(() => api.openPreviewInBrowser(project.id))}>Open in browser</button>
          </div>
        )}
        <p className="muted small">
          Uses the project's local Remotion CLI with the same workspace and dependencies as rendering. The server listens on localhost only and is stopped when you close the preview or the app.
        </p>
        {project.report.compositions.length > 0 && (
          <>
            <h2 className="mt">Compositions</h2>
            <ul className="plain mono">{project.report.compositions.map((c) => <li key={c.id}>{compositionLabel(c)}</li>)}</ul>
          </>
        )}
      </section>
      {info?.error && <DiagnosticBox d={info.error} />}
      <section className="card">
        <h2>Preview log</h2>
        <LogView lines={data.logTail(key)} />
      </section>
    </>
  );
}

const CODECS: [string, string][] = [
  ["h264", "H.264 (.mp4)"],
  ["h265", "H.265 (.mp4)"],
  ["vp8", "VP8 (.webm)"],
  ["vp9", "VP9 (.webm)"],
  ["prores", "ProRes (.mov)"],
  ["gif", "GIF (.gif)"],
];
const EXT: Record<string, string> = { h264: "mp4", h265: "mp4", vp8: "webm", vp9: "webm", prores: "mov", gif: "gif" };

function Render({ project, data, guard, go }: { project: Project; data: AppData; guard: Guard; go: (r: Route) => void }) {
  const comps = project.report.compositions;
  const [composition, setComposition] = useState(comps[0]?.id ?? "");
  const [codec, setCodec] = useState("h264");
  const [fileName, setFileName] = useState(`${project.name.replace(/[<>:"/\\|?*]/g, "_")}.mp4`);
  const [outputDir, setOutputDir] = useState(data.settings?.outputDir ?? "");
  const [scale, setScale] = useState("1");
  const [concurrency, setConcurrency] = useState("");
  const [frameStart, setFrameStart] = useState("");
  const [frameEnd, setFrameEnd] = useState("");
  const [overwrite, setOverwrite] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const ready = project.envStatus === "ready";
  const cores = navigator.hardwareConcurrency || 4;

  useEffect(() => {
    if (!composition && comps[0]) setComposition(comps[0].id);
  }, [comps, composition]);

  const changeCodec = (c: string) => {
    setCodec(c);
    setFileName((n) => `${n.replace(/\.[A-Za-z0-9]+$/, "")}.${EXT[c]}`);
  };
  const browse = async () => {
    const dir = await open({ directory: true, defaultPath: outputDir || undefined });
    if (typeof dir === "string") setOutputDir(dir);
  };
  const detect = async () => {
    setDetecting(true);
    await guard(async () => {
      const found = await api.detectCompositions(project.id);
      if (found[0] && !found.some((c) => c.id === composition)) setComposition(found[0].id);
    });
    setDetecting(false);
  };
  const num = (v: string) => (v.trim() === "" ? null : Number(v));
  const queue = () =>
    guard(() =>
      api.enqueueRender({
        projectId: project.id,
        compositionId: composition.trim(),
        outputDir,
        fileName,
        codec,
        scale: num(scale),
        concurrency: num(concurrency),
        frameStart: num(frameStart),
        frameEnd: num(frameEnd),
        overwrite,
      }),
    );

  const selected = comps.find((c) => c.id === composition);
  const job = useMemo(() => data.jobs.find((j) => j.projectId === project.id), [data.jobs, project.id]);
  const live = job ? data.progress[job.id] : undefined;
  const frame = live?.frame ?? job?.frame ?? 0;
  const total = live?.total ?? job?.totalFrames ?? 0;
  const active = job && (job.status === "rendering" || job.status === "preparing");
  const highConcurrency = num(concurrency) != null && (num(concurrency) as number) > Math.max(1, Math.floor(cores / 2));

  return (
    <div className="grid-2">
      <section className="card">
        <h2>Render settings</h2>
        {!ready && (
          <p className="warn-text">
            The environment is not ready.{" "}
            <button className="link" onClick={() => go({ page: "project", id: project.id, tab: "environment" })}>Prepare it first</button>.
          </p>
        )}
        <Field label="Composition" hint={selected ? compositionLabel(selected) : "Type the ID exactly as registered in the project."}>
          <div className="inline">
            <input list="compositions" value={composition} onChange={(e) => setComposition(e.target.value)} placeholder="Composition ID" />
            <datalist id="compositions">{comps.map((c) => <option key={c.id} value={c.id} />)}</datalist>
            <button className="btn" disabled={!ready || detecting} onClick={detect}>{detecting ? "Detecting..." : "Detect compositions"}</button>
          </div>
        </Field>
        <Field label="Output folder">
          <div className="inline">
            <input value={outputDir} onChange={(e) => setOutputDir(e.target.value)} />
            <button className="btn" onClick={browse}>Browse...</button>
          </div>
        </Field>
        <div className="inline-fields">
          <Field label="File name">
            <input value={fileName} onChange={(e) => setFileName(e.target.value)} />
          </Field>
          <Field label="Codec / container">
            <select value={codec} onChange={(e) => changeCodec(e.target.value)}>
              {CODECS.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
            </select>
          </Field>
        </div>
        <div className="inline-fields">
          <Field label="Scale" hint={selected?.width && Number(scale) > 0 ? `${Math.round(selected.width * Number(scale))}x${Math.round((selected.height ?? 0) * Number(scale))}` : "1 = original size"}>
            <input type="number" min="0.1" max="16" step="0.1" value={scale} onChange={(e) => setScale(e.target.value)} />
          </Field>
          <Field label="Concurrency" hint={`Blank = Remotion default. This PC has ${cores} threads.`}>
            <input type="number" min="1" max={cores} value={concurrency} onChange={(e) => setConcurrency(e.target.value)} placeholder="auto" />
          </Field>
          <Field label="First frame" hint="Optional">
            <input type="number" min="0" value={frameStart} onChange={(e) => setFrameStart(e.target.value)} />
          </Field>
          <Field label="Last frame" hint="Optional">
            <input type="number" min="0" value={frameEnd} onChange={(e) => setFrameEnd(e.target.value)} />
          </Field>
        </div>
        {highConcurrency && <p className="warn-text">High concurrency uses a lot of CPU and RAM and can make the render fail on low-memory machines.</p>}
        <label className="check">
          <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} />
          <span>Replace an existing file with the same name (otherwise a numbered copy is written)</span>
        </label>
        <div className="card-actions">
          <button className="btn btn-primary" disabled={!ready || !composition.trim() || !outputDir.trim()} onClick={queue}>Add to render queue</button>
          <button className="btn" onClick={() => go({ page: "queue" })}>Open render queue</button>
        </div>
        <p className="muted small">The video is written to a temporary <code>.partial</code> file and only gets its final name after the render succeeds and the file is verified.</p>
      </section>
      <section className="card">
        <div className="card-head">
          <h2>Latest render</h2>
          {job && <JobBadge status={job.status} />}
          <span className="spacer" />
          {job && (active || job.status === "waiting") && <button className="btn btn-danger btn-small" onClick={() => guard(() => api.cancelJob(job.id))}>Cancel render</button>}
          {job?.status === "completed" && <button className="btn btn-small" onClick={() => guard(() => api.revealJobOutput(job.id))}>Show in folder</button>}
        </div>
        {job ? (
          <>
            <p className="mono small">{job.compositionId} &rarr; {job.outputPath}</p>
            {active && (
              <>
                <ProgressBar value={total ? frame / total : 0} indeterminate={!total} />
                <p className="muted small">{live?.phase ?? job.phase}{total ? ` · frame ${frame} / ${total}` : ""}</p>
              </>
            )}
            {job.status === "completed" && <p className="muted small">{bytes(job.outputBytes)} · finished {when(job.finishedAt)}</p>}
            {job.error && <DiagnosticBox d={job.error} />}
            <LogView lines={data.logTail(`job:${job.id}`)} empty="Live output appears here while rendering. Earlier logs are in the Logs tab." />
          </>
        ) : (
          <p className="muted">No renders yet for this project.</p>
        )}
      </section>
    </div>
  );
}

function Logs({ project, guard }: { project: Project; guard: Guard }) {
  const [files, setFiles] = useState<LogFile[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [text, setText] = useState("");
  const refresh = () => api.listLogs(project.id).then(setFiles).catch(() => setFiles([]));
  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project.id]);
  const show = (name: string) => {
    setSelected(name);
    void guard(async () => setText(await api.readLog(project.id, name)));
  };
  const remove = () =>
    guard(async () => {
      await api.deleteLogs(project.id);
      setSelected(null);
      setText("");
      await refresh();
    });
  return (
    <div className="grid-logs">
      <section className="card">
        <div className="card-head">
          <h2>Log files</h2>
          <span className="spacer" />
          <button className="btn btn-small" onClick={refresh}>Refresh</button>
          <button className="btn btn-small btn-danger" disabled={!files.length} onClick={remove}>Delete logs</button>
        </div>
        {files.length ? (
          <ul className="file-list">
            {files.map((f) => (
              <li key={f.name}>
                <button className={`file${selected === f.name ? " active" : ""}`} onClick={() => show(f.name)}>
                  <span className="mono">{f.name}</span>
                  <span className="muted small">{bytes(f.bytes)} · {when(f.modified)}</span>
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="muted">No logs yet.</p>
        )}
        <p className="muted small">Kept: the last 10 render logs, and nothing older than 14 days. Secrets and tokens are masked.</p>
      </section>
      <section className="card">
        <h2>{selected ?? "Select a log"}</h2>
        <LogView lines={text ? text.split("\n") : []} empty="Choose a log file on the left." />
      </section>
    </div>
  );
}
