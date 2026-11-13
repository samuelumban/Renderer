import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { DiagnosticBox } from "../components/common";
import { api, inTauri, toDiagnostic } from "../services/api";
import type { Diagnostic, Route } from "../types";

export default function ImportPage({ go }: { go: (r: Route) => void }) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<Diagnostic | null>(null);
  const [hover, setHover] = useState(false);
  const nameRef = useRef(name);
  nameRef.current = name;
  const busyRef = useRef(false);

  const run = async (path: string) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(path);
    setError(null);
    try {
      const project = await api.importZip(path, nameRef.current.trim() || undefined);
      go({ page: "project", id: project.id, tab: "overview" });
    } catch (e) {
      setError(toDiagnostic(e));
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  };
  const runRef = useRef(run);
  runRef.current = run;

  useEffect(() => {
    if (!inTauri) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "enter" || p.type === "over") setHover(true);
        else if (p.type === "leave") setHover(false);
        else if (p.type === "drop") {
          setHover(false);
          const zip = p.paths.find((f) => f.toLowerCase().endsWith(".zip"));
          if (zip) void runRef.current(zip);
          else setError({ summary: "Only .zip files can be imported.", causeIsHypothesis: false });
        }
      })
      .then((u) => (disposed ? u() : (unlisten = u)));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const choose = async () => {
    const picked = await open({ multiple: false, directory: false, filters: [{ name: "Remotion project ZIP", extensions: ["zip"] }] });
    if (typeof picked === "string") void run(picked);
  };

  return (
    <>
      <header className="page-head">
        <div>
          <h1>Import ZIP</h1>
          <p className="muted">The archive is checked and extracted into the app workspace. Nothing in it is executed during import.</p>
        </div>
      </header>
      {error && <DiagnosticBox d={error} onDismiss={() => setError(null)} />}
      <div className={`dropzone${hover ? " hover" : ""}${busy ? " busy" : ""}`}>
        {busy ? (
          <>
            <div className="spinner" aria-hidden="true" />
            <h2>Importing...</h2>
            <p className="muted mono small">{busy}</p>
            <button className="btn" onClick={() => api.cancelImport()}>Cancel</button>
          </>
        ) : (
          <>
            <h2>Drop a Remotion project .zip here</h2>
            <p className="muted">or</p>
            <button className="btn btn-primary" onClick={choose}>Choose ZIP file</button>
          </>
        )}
      </div>
      <div className="card form-narrow">
        <label className="field">
          <span className="field-label">Project name (optional)</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Taken from package.json or the ZIP name" disabled={!!busy} />
        </label>
      </div>
      <ul className="notes muted">
        <li>Paths that try to leave the project folder, and symbolic links, are rejected.</li>
        <li><code>node_modules</code> inside the ZIP is skipped; dependencies are installed from the project's lockfile.</li>
        <li>Your original ZIP is never modified or deleted.</li>
      </ul>
    </>
  );
}
