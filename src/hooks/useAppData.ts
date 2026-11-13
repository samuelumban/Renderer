import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../services/api";
import type { PreviewInfo, Progress, Project, RenderJob, Settings } from "../types";

const MAX_LOG_LINES = 500;

export interface AppData {
  ready: boolean;
  projects: Project[];
  jobs: RenderJob[];
  previews: PreviewInfo[];
  settings: Settings | null;
  progress: Record<string, Progress>;
  /** Bounded tail of live log lines for a key ("env:<id>", "preview:<id>", "job:<id>"). */
  logTail: (key: string) => string[];
  clearLog: (key: string) => void;
  refreshSettings: () => Promise<void>;
}

export function useAppData(): AppData {
  const [ready, setReady] = useState(false);
  const [projects, setProjects] = useState<Project[]>([]);
  const [jobs, setJobs] = useState<RenderJob[]>([]);
  const [previews, setPreviews] = useState<PreviewInfo[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const logs = useRef<Map<string, string[]>>(new Map());
  const [, setLogVersion] = useState(0);
  const pending = useRef(false);

  const refreshSettings = useCallback(async () => setSettings(await api.getSettings()), []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const load = {
      projects: () => api.listProjects().then((p) => !disposed && setProjects(p)),
      jobs: () => api.listJobs().then((j) => !disposed && setJobs(j)),
      previews: () => api.listPreviews().then((p) => !disposed && setPreviews(p)),
    };
    api
      .onEvent((e) => {
        switch (e.type) {
          case "projects":
            void load.projects();
            break;
          case "jobs":
            void load.jobs();
            break;
          case "previews":
            void load.previews();
            break;
          case "progress":
            setProgress((p) => ({ ...p, [e.jobId]: { frame: e.frame, total: e.total, phase: e.phase } }));
            break;
          case "log": {
            // Keep only a bounded tail in memory; full logs live on disk.
            const lines = logs.current.get(e.key) ?? [];
            lines.push(e.line);
            if (lines.length > MAX_LOG_LINES) lines.splice(0, lines.length - MAX_LOG_LINES);
            logs.current.set(e.key, lines);
            if (!pending.current) {
              pending.current = true;
              requestAnimationFrame(() => {
                pending.current = false;
                setLogVersion((v) => v + 1);
              });
            }
            break;
          }
        }
      })
      .then((u) => {
        if (disposed) u();
        else unlisten = u;
      });
    Promise.all([load.projects(), load.jobs(), load.previews(), refreshSettings()]).finally(() => !disposed && setReady(true));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshSettings]);

  const logTail = useCallback((key: string) => logs.current.get(key) ?? [], []);
  const clearLog = useCallback((key: string) => {
    logs.current.delete(key);
    setLogVersion((v) => v + 1);
  }, []);

  return { ready, projects, jobs, previews, settings, progress, logTail, clearLog, refreshSettings };
}

/** Re-render every second while `active` (elapsed-time displays). */
export function useTicker(active: boolean): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [active]);
  return now;
}
