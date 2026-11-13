import { useEffect, useState } from "react";
import { DiagnosticBox, EnvBadge, Modal } from "../components/common";
import type { AppData } from "../hooks/useAppData";
import { api, toDiagnostic } from "../services/api";
import { bytes, when } from "../services/format";
import type { Diagnostic, Project, Route } from "../types";

export default function ProjectsPage({ data, go }: { data: AppData; go: (r: Route) => void }) {
  const [sizes, setSizes] = useState<Record<string, number>>({});
  const [removing, setRemoving] = useState<Project | null>(null);
  const [error, setError] = useState<Diagnostic | null>(null);
  const count = data.projects.length;

  useEffect(() => {
    api.projectSizes().then(setSizes).catch(() => {});
  }, [count]);

  const open = (p: Project, tab: "overview" | "preview" | "render" | "environment" = "overview") => {
    void api.openProject(p.id).catch(() => {});
    go({ page: "project", id: p.id, tab });
  };

  const remove = async (p: Project) => {
    setRemoving(null);
    try {
      await api.removeProject(p.id);
    } catch (e) {
      setError(toDiagnostic(e));
    }
  };

  return (
    <>
      <header className="page-head">
        <div>
          <h1>Projects</h1>
          <p className="muted">Each project runs with its own Remotion version, lockfile and local CLI.</p>
        </div>
        <button className="btn btn-primary" onClick={() => go({ page: "import" })}>Import ZIP</button>
      </header>
      {error && <DiagnosticBox d={error} onDismiss={() => setError(null)} />}
      {count === 0 ? (
        <div className="empty">
          <h2>No projects yet</h2>
          <p className="muted">Import a Remotion project ZIP to inspect, preview and render it.</p>
          <button className="btn btn-primary" onClick={() => go({ page: "import" })}>Import ZIP</button>
        </div>
      ) : (
        <div className="card table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Project</th>
                <th>Remotion</th>
                <th>Node</th>
                <th>Environment</th>
                <th>Last opened</th>
                <th>Last render</th>
                <th className="num">Storage</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {data.projects.map((p) => (
                <tr key={p.id}>
                  <td>
                    <button className="link strong" onClick={() => open(p)}>{p.name}</button>
                    <div className="muted small">{p.packageManager}{p.storageMode === "save-space" ? " · Save Space" : ""}</div>
                  </td>
                  <td className="mono">{p.remotionVersion ?? "-"}</td>
                  <td className="mono">{p.nodeVersion ? `v${p.nodeVersion}` : p.nodeRequirement ?? "default"}</td>
                  <td><EnvBadge status={p.envStatus} /></td>
                  <td>{when(p.lastOpenedAt)}</td>
                  <td>{when(p.lastRenderAt)}</td>
                  <td className="num">{bytes(sizes[p.id])}</td>
                  <td>
                    <div className="row-actions">
                      <button className="btn btn-small" onClick={() => open(p)}>Open</button>
                      <button className="btn btn-small" onClick={() => open(p, "preview")}>Preview</button>
                      <button className="btn btn-small" onClick={() => open(p, "render")}>Render</button>
                      <button className="btn btn-small" onClick={() => open(p, "environment")}>Rebuild</button>
                      <button className="btn btn-small btn-danger" onClick={() => setRemoving(p)}>Remove</button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {removing && (
        <Modal title={`Remove "${removing.name}"?`} onClose={() => setRemoving(null)}>
          <p>This deletes the project's workspace, installed dependencies and logs from app storage.</p>
          <p className="muted">Your original ZIP ({removing.originalZipPath}) and rendered videos are not touched.</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setRemoving(null)}>Cancel</button>
            <button className="btn btn-danger" onClick={() => remove(removing)}>Remove project</button>
          </div>
        </Modal>
      )}
    </>
  );
}
