import { useEffect, useState } from "react";
import { DiagnosticBox } from "../components/common";
import type { AppData } from "../hooks/useAppData";
import { api, toDiagnostic } from "../services/api";
import { bytes } from "../services/format";
import type { Diagnostic, StorageUsage } from "../types";

export default function StoragePage({ data }: { data: AppData }) {
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<Diagnostic | null>(null);

  const refresh = () => api.storageUsage().then(setUsage).catch((e) => setError(toDiagnostic(e)));
  useEffect(() => {
    void refresh();
  }, [data.projects.length, data.jobs.length]);

  const run = async (label: string, action: () => Promise<number>) => {
    setBusy(label);
    setError(null);
    setMessage(null);
    try {
      const freed = await action();
      setMessage(`${label}: freed ${bytes(freed)}.`);
    } catch (e) {
      setError(toDiagnostic(e));
    }
    setBusy(null);
    void refresh();
  };

  const rows: [string, number, string][] = usage
    ? [
        ["Project workspaces", usage.workspaces, "Extracted sources and node_modules"],
        ["Node runtimes", usage.runtimes, "App-managed Node.js, pnpm and Yarn"],
        ["Package cache", usage.packageCache, `Limit ${bytes(usage.cacheLimitBytes)}; oldest entries are removed first`],
        ["Browser cache", usage.browserCache, "Rendering browser downloaded by Remotion"],
        ["Temporary files", usage.temp, "Removed automatically after each job"],
        ["Logs", usage.logs, "Last 10 jobs / 14 days per project"],
      ]
    : [];
  const total = rows.reduce((s, r) => s + r[1], 0);

  return (
    <>
      <header className="page-head">
        <div>
          <h1>Storage</h1>
          <p className="muted">Everything the app stores lives in one folder. Your ZIP files and rendered videos are never part of cleanup.</p>
        </div>
        <button className="btn btn-primary" disabled={!!busy} onClick={() => run("Clean now", api.cleanNow)}>Clean now</button>
      </header>
      {error && <DiagnosticBox d={error} onDismiss={() => setError(null)} />}
      {message && <p className="ok-text">{message}</p>}
      <section className="card">
        {!usage ? (
          <p className="muted">Measuring...</p>
        ) : (
          <table className="table storage">
            <tbody>
              {rows.map(([label, value, hint]) => (
                <tr key={label}>
                  <td>
                    <strong>{label}</strong>
                    <div className="muted small">{hint}</div>
                  </td>
                  <td className="bar-cell">
                    <div className="bar"><div className="bar-fill" style={{ width: `${total ? Math.max(value ? 1 : 0, (value / total) * 100) : 0}%` }} /></div>
                  </td>
                  <td className="num mono">{bytes(value)}</td>
                </tr>
              ))}
              <tr className="total">
                <td><strong>Total</strong></td>
                <td />
                <td className="num mono">{bytes(total)}</td>
              </tr>
            </tbody>
          </table>
        )}
        {usage && (
          <p className="muted small">
            {usage.freeBytes != null && <>Free on this drive: {bytes(usage.freeBytes)} · </>}
            <span className="mono">{usage.baseDir}</span>{" "}
            <button className="link" onClick={() => api.openDataFolder().catch((e) => setError(toDiagnostic(e)))}>Open folder</button>
          </p>
        )}
      </section>
      <section className="card">
        <h2>Cleanup</h2>
        <div className="actions-grid">
          <div>
            <button className="btn" disabled={!!busy} onClick={() => run("Clean temporary files", api.cleanTemp)}>Clean temporary files</button>
            <p className="muted small">Leftover render and preview files. Skips projects that are busy.</p>
          </div>
          <div>
            <button className="btn" disabled={!!busy} onClick={() => run("Clear package cache", api.clearPackageCache)}>Clear package cache</button>
            <p className="muted small">Installed projects keep working; the next install downloads packages again.</p>
          </div>
          <div>
            <button className="btn" disabled={!!busy} onClick={() => run("Remove unused runtimes", api.removeUnusedRuntimes)}>Remove unused runtimes</button>
            <p className="muted small">Node.js versions no project uses. Runtimes in use are kept.</p>
          </div>
          <div>
            <button className="btn" disabled={!!busy} onClick={() => run("Delete logs", () => api.deleteLogs())}>Delete logs</button>
            <p className="muted small">All diagnostic logs for all projects.</p>
          </div>
        </div>
        <p className="muted small">Caches and runtimes are never cleared while an install or render is running.</p>
      </section>
    </>
  );
}
