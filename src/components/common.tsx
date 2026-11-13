import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Diagnostic, EnvStatus, JobStatus } from "../types";

export function Badge({ tone, children }: { tone: "ok" | "warn" | "bad" | "info" | "muted"; children: ReactNode }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

const ENV: Record<EnvStatus, [string, "ok" | "warn" | "bad" | "info" | "muted"]> = {
  "not-prepared": ["Not prepared", "muted"],
  preparing: ["Preparing...", "info"],
  ready: ["Ready", "ok"],
  failed: ["Failed", "bad"],
  removed: ["Removed", "warn"],
};

export function EnvBadge({ status }: { status: EnvStatus }) {
  const [label, tone] = ENV[status];
  return <span title={status === "removed" ? "Workspace removed by Save Space mode; the ZIP and report are kept" : undefined}><Badge tone={tone}>{label}</Badge></span>;
}

const JOB: Record<JobStatus, [string, "ok" | "warn" | "bad" | "info" | "muted"]> = {
  waiting: ["Waiting", "muted"],
  preparing: ["Preparing", "info"],
  rendering: ["Rendering", "info"],
  completed: ["Completed", "ok"],
  failed: ["Failed", "bad"],
  cancelled: ["Cancelled", "warn"],
};

export function JobBadge({ status }: { status: JobStatus }) {
  const [label, tone] = JOB[status];
  return <Badge tone={tone}>{label}</Badge>;
}

export function diagnosticText(d: Diagnostic): string {
  return [
    `Summary: ${d.summary}`,
    d.command ? `Command: ${d.command}` : "",
    d.exitCode != null ? `Exit code: ${d.exitCode}` : "",
    d.likelyCause ? `Likely cause${d.causeIsHypothesis ? " (hypothesis)" : ""}: ${d.likelyCause}` : "",
    d.suggestion ? `Suggested action: ${d.suggestion}` : "",
    d.excerpt ? `Output:\n${d.excerpt}` : "",
  ]
    .filter(Boolean)
    .join("\n");
}

/** Error presentation: summary, command + exit code, excerpt, cause, action, copy. */
export function DiagnosticBox({ d, onDismiss }: { d: Diagnostic; onDismiss?: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(diagnosticText(d));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };
  return (
    <div className="diag" role="alert">
      <div className="diag-head">
        <strong>{d.summary}</strong>
        <span className="spacer" />
        <button className="btn btn-small" onClick={copy}>{copied ? "Copied" : "Copy diagnostics"}</button>
        {onDismiss && <button className="btn btn-small" onClick={onDismiss}>Dismiss</button>}
      </div>
      {d.likelyCause && (
        <p>
          <span className="diag-label">{d.causeIsHypothesis ? "Possible cause (not certain)" : "Cause"}</span>
          {d.likelyCause}
        </p>
      )}
      {d.suggestion && (
        <p>
          <span className="diag-label">What to do</span>
          {d.suggestion}
        </p>
      )}
      {d.command && (
        <p className="mono small">
          <span className="diag-label">Command</span>
          {d.command}
          {d.exitCode != null && ` (exit code ${d.exitCode})`}
        </p>
      )}
      {d.excerpt && <pre className="diag-excerpt">{d.excerpt}</pre>}
    </div>
  );
}

export function LogView({ lines, empty }: { lines: string[]; empty?: string }) {
  const ref = useRef<HTMLPreElement>(null);
  const stick = useRef(true);
  useEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  });
  return (
    <pre
      className="log"
      ref={ref}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
      }}
    >
      {lines.length ? lines.join("\n") : <span className="muted">{empty ?? "No output yet."}</span>}
    </pre>
  );
}

export function Modal({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true" aria-label={title}>
        <h2>{title}</h2>
        {children}
      </div>
    </div>
  );
}

export function ProgressBar({ value, indeterminate }: { value: number; indeterminate?: boolean }) {
  return (
    <div className={`progress${indeterminate ? " progress-indeterminate" : ""}`}>
      <div className="progress-fill" style={{ width: `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%` }} />
    </div>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}
