import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { DiagnosticBox, Field, Modal } from "../components/common";
import type { AppData } from "../hooks/useAppData";
import { api, toDiagnostic } from "../services/api";
import type { Diagnostic, Settings } from "../types";

const GB = 1024 ** 3;
const MB = 1024 ** 2;

export default function SettingsPage({ data }: { data: AppData }) {
  const [s, setS] = useState<Settings | null>(data.settings);
  const [error, setError] = useState<Diagnostic | null>(null);
  const [saved, setSaved] = useState(false);
  const [confirmRenders, setConfirmRenders] = useState<number | null>(null);

  useEffect(() => {
    if (!s && data.settings) setS(data.settings);
  }, [data.settings, s]);
  if (!s) return <p className="muted">Loading...</p>;

  const set = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSaved(false);
    setS({ ...s, [key]: value });
  };
  const save = async () => {
    setError(null);
    try {
      setS(await api.updateSettings(s));
      await data.refreshSettings();
      setSaved(true);
    } catch (e) {
      setError(toDiagnostic(e));
    }
  };
  const pickDir = async () => {
    const dir = await open({ directory: true, defaultPath: s.outputDir });
    if (typeof dir === "string") set("outputDir", dir);
  };
  const pickBrowser = async () => {
    const f = await open({ multiple: false, filters: [{ name: "Executable", extensions: ["exe"] }] });
    if (typeof f === "string") set("browserExecutable", f);
  };
  const number = (v: string, fallback: number) => (Number.isFinite(Number(v)) && v !== "" ? Number(v) : fallback);

  return (
    <>
      <header className="page-head">
        <div>
          <h1>Settings</h1>
          <p className="muted">Defaults for new projects and limits for app storage.</p>
        </div>
        <div className="inline">
          {saved && <span className="ok-text">Saved</span>}
          <button className="btn btn-primary" onClick={save}>Save settings</button>
        </div>
      </header>
      {error && <DiagnosticBox d={error} onDismiss={() => setError(null)} />}
      <div className="grid-2">
        <section className="card">
          <h2>Output</h2>
          <Field label="Default output folder" hint="Final videos are written here unless you choose another folder per render.">
            <div className="inline">
              <input value={s.outputDir} onChange={(e) => set("outputDir", e.target.value)} />
              <button className="btn" onClick={pickDir}>Browse...</button>
            </div>
          </Field>
          <Field label="Renders at the same time" hint="1 is recommended. Each render starts its own browser and encoder.">
            <select
              value={s.maxConcurrentRenders}
              onChange={(e) => {
                const n = Number(e.target.value);
                if (n > 1 && n > s.maxConcurrentRenders) setConfirmRenders(n);
                else set("maxConcurrentRenders", n);
              }}
            >
              {[1, 2, 3, 4].map((n) => <option key={n} value={n}>{n}</option>)}
            </select>
          </Field>
          <Field label="Browser executable (optional)" hint="Leave empty to let each project's Remotion version manage its own rendering browser.">
            <div className="inline">
              <input value={s.browserExecutable ?? ""} onChange={(e) => set("browserExecutable", e.target.value || null)} placeholder="Managed by Remotion" />
              <button className="btn" onClick={pickBrowser}>Browse...</button>
            </div>
          </Field>
        </section>
        <section className="card">
          <h2>Storage</h2>
          <Field label="Default storage mode for new projects">
            <select value={s.defaultStorageMode} onChange={(e) => set("defaultStorageMode", e.target.value as Settings["defaultStorageMode"])}>
              <option value="fast-reopen">Fast Reopen — keep workspace and node_modules</option>
              <option value="save-space">Save Space — delete workspace after rendering</option>
            </select>
          </Field>
          <label className="check">
            <input type="checkbox" checked={s.keepZipCopy} onChange={(e) => set("keepZipCopy", e.target.checked)} />
            <span>Keep a copy of each imported ZIP in app storage (lets Save Space projects reopen even if the original file moves)</span>
          </label>
          <Field label="Package cache limit (GB)" hint="When exceeded, the least recently used cache entries are removed.">
            <input type="number" min="0.1" step="0.5" value={+(s.cacheLimitBytes / GB).toFixed(2)} onChange={(e) => set("cacheLimitBytes", Math.round(number(e.target.value, 5) * GB))} />
          </Field>
        </section>
        <section className="card">
          <h2>Node.js runtime</h2>
          <Field label="Default Node.js major version" hint="Used when a project does not state engines.node. Runtimes are downloaded on demand into app storage.">
            <input type="number" min="14" max="99" value={s.defaultNodeMajor} onChange={(e) => set("defaultNodeMajor", number(e.target.value, 22))} />
          </Field>
          <Field label="Node.js download mirror">
            <input value={s.nodeMirror} onChange={(e) => set("nodeMirror", e.target.value)} />
          </Field>
          <Field label="Install timeout (minutes)">
            <input type="number" min="1" max="240" value={s.installTimeoutMinutes} onChange={(e) => set("installTimeoutMinutes", number(e.target.value, 30))} />
          </Field>
        </section>
        <section className="card">
          <h2>Import limits</h2>
          <Field label="Maximum ZIP size (MB)">
            <input type="number" min="1" value={Math.round(s.maxZipBytes / MB)} onChange={(e) => set("maxZipBytes", number(e.target.value, 1024) * MB)} />
          </Field>
          <Field label="Maximum extracted size (MB)" hint="Protects against archives that expand to enormous sizes.">
            <input type="number" min="1" value={Math.round(s.maxExtractedBytes / MB)} onChange={(e) => set("maxExtractedBytes", number(e.target.value, 4096) * MB)} />
          </Field>
          <Field label="Maximum number of files">
            <input type="number" min="1" value={s.maxZipEntries} onChange={(e) => set("maxZipEntries", number(e.target.value, 200000))} />
          </Field>
        </section>
      </div>
      {confirmRenders && (
        <Modal title="Run several renders at once?" onClose={() => setConfirmRenders(null)}>
          <p>Each render uses multiple CPU threads and can need several gigabytes of RAM. Running {confirmRenders} at once may slow the computer down or make renders fail.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmRenders(null)}>Keep current setting</button>
            <button
              className="btn btn-primary"
              onClick={() => {
                set("maxConcurrentRenders", confirmRenders);
                setConfirmRenders(null);
              }}
            >
              Allow {confirmRenders} renders
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}
