import { useState } from "react";
import { DiagnosticBox, JobBadge, ProgressBar } from "../components/common";
import { useTicker, type AppData } from "../hooks/useAppData";
import { api, toDiagnostic } from "../services/api";
import { bytes, duration, when } from "../services/format";
import type { Diagnostic, RenderJob, Route } from "../types";

const finished = (j: RenderJob) => ["completed", "failed", "cancelled"].includes(j.status);

export default function QueuePage({ data, go }: { data: AppData; go: (r: Route) => void }) {
  const [error, setError] = useState<Diagnostic | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const anyActive = data.jobs.some((j) => j.status === "rendering" || j.status === "preparing");
  const now = useTicker(anyActive);
  const guard = (f: () => Promise<unknown>) => f().catch((e) => setError(toDiagnostic(e)));
  const max = data.settings?.maxConcurrentRenders ?? 1;

  return (
    <>
      <header className="page-head">
        <div>
          <h1>Render Queue</h1>
          <p className="muted">
            {max === 1 ? "One render runs at a time to avoid memory pressure." : `Up to ${max} renders run at the same time.`} Completed jobs keep only the final video and a short record.
          </p>
        </div>
        <button className="btn" disabled={!data.jobs.some(finished)} onClick={() => guard(api.clearFinishedJobs)}>Clear finished</button>
      </header>
      {error && <DiagnosticBox d={error} onDismiss={() => setError(null)} />}
      {data.jobs.length === 0 ? (
        <div className="empty">
          <h2>The queue is empty</h2>
          <p className="muted">Open a project and use its Render tab to add a job.</p>
        </div>
      ) : (
        <div className="jobs">
          {data.jobs.map((j) => {
            const live = data.progress[j.id];
            const frame = live?.frame ?? j.frame;
            const total = live?.total ?? j.totalFrames;
            const active = j.status === "rendering" || j.status === "preparing";
            const elapsed = j.startedAt ? (j.finishedAt ? new Date(j.finishedAt).getTime() : now) - new Date(j.startedAt).getTime() : 0;
            return (
              <section className="card job" key={j.id}>
                <div className="card-head">
                  <button className="link strong" onClick={() => go({ page: "project", id: j.projectId, tab: "render" })}>{j.projectName}</button>
                  <span className="mono muted">{j.compositionId}</span>
                  <JobBadge status={j.status} />
                  <span className="spacer" />
                  {j.startedAt && <span className="muted small">{duration(elapsed)}</span>}
                  {!finished(j) && <button className="btn btn-small btn-danger" onClick={() => guard(() => api.cancelJob(j.id))}>Cancel</button>}
                  {j.status === "completed" && <button className="btn btn-small" onClick={() => guard(() => api.revealJobOutput(j.id))}>Show in folder</button>}
                  {j.error && <button className="btn btn-small" onClick={() => setExpanded(expanded === j.id ? null : j.id)}>{expanded === j.id ? "Hide details" : "Details"}</button>}
                </div>
                <p className="mono small">{j.outputPath}</p>
                {active && (
                  <>
                    <ProgressBar value={total ? frame / total : 0} indeterminate={!total} />
                    <p className="muted small">{live?.phase ?? j.phase}{total ? ` · frame ${frame} / ${total}` : ""}</p>
                  </>
                )}
                {j.status === "completed" && <p className="muted small">{bytes(j.outputBytes)} · {j.totalFrames ? `${j.totalFrames} frames · ` : ""}finished {when(j.finishedAt)}</p>}
                {j.status === "failed" && !j.error && <p className="warn-text">Failed.</p>}
                {j.error && expanded !== j.id && <p className="warn-text">{j.error.summary}</p>}
                {j.error && expanded === j.id && <DiagnosticBox d={j.error} />}
              </section>
            );
          })}
        </div>
      )}
    </>
  );
}
