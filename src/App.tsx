import { useState } from "react";
import { useAppData } from "./hooks/useAppData";
import type { Route } from "./types";
import ProjectsPage from "./pages/ProjectsPage";
import ImportPage from "./pages/ImportPage";
import ProjectPage from "./pages/ProjectPage";
import QueuePage from "./pages/QueuePage";
import StoragePage from "./pages/StoragePage";
import SettingsPage from "./pages/SettingsPage";

export default function App() {
  const data = useAppData();
  const [route, setRoute] = useState<Route>({ page: "projects" });
  const [lastProject, setLastProject] = useState<string | null>(null);

  const go = (r: Route) => {
    if (r.page === "project") setLastProject(r.id);
    setRoute(r);
  };
  const current = lastProject && data.projects.some((p) => p.id === lastProject) ? lastProject : null;
  const activeJobs = data.jobs.filter((j) => !["completed", "failed", "cancelled"].includes(j.status)).length;
  const isProject = (tab?: string) => route.page === "project" && (!tab || route.tab === tab);

  const nav: { label: string; active: boolean; onClick: () => void; disabled?: boolean; badge?: number }[] = [
    { label: "Projects", active: route.page === "projects" || (isProject() && !isProject("preview") && !isProject("render")), onClick: () => go({ page: "projects" }) },
    { label: "Import ZIP", active: route.page === "import", onClick: () => go({ page: "import" }) },
    { label: "Preview", active: isProject("preview"), disabled: !current, onClick: () => current && go({ page: "project", id: current, tab: "preview" }) },
    { label: "Render", active: isProject("render"), disabled: !current, onClick: () => current && go({ page: "project", id: current, tab: "render" }) },
    { label: "Render Queue", active: route.page === "queue", onClick: () => go({ page: "queue" }), badge: activeJobs },
    { label: "Storage", active: route.page === "storage", onClick: () => go({ page: "storage" }) },
    { label: "Settings", active: route.page === "settings", onClick: () => go({ page: "settings" }) },
  ];

  const project = route.page === "project" ? data.projects.find((p) => p.id === route.id) : undefined;

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark" aria-hidden="true" />
          <div>
            <div className="brand-name">Remotion Studio</div>
            <div className="brand-sub">Universal render manager</div>
          </div>
        </div>
        <nav>
          {nav.map((n) => (
            <button
              key={n.label}
              className={`nav-item${n.active ? " active" : ""}`}
              disabled={n.disabled}
              title={n.disabled ? "Open a project first" : undefined}
              onClick={n.onClick}
            >
              {n.label}
              {!!n.badge && <span className="nav-badge">{n.badge}</span>}
            </button>
          ))}
        </nav>
      </aside>
      <main className="content">
        {!data.ready ? (
          <p className="muted">Loading...</p>
        ) : route.page === "projects" ? (
          <ProjectsPage data={data} go={go} />
        ) : route.page === "import" ? (
          <ImportPage go={go} />
        ) : route.page === "queue" ? (
          <QueuePage data={data} go={go} />
        ) : route.page === "storage" ? (
          <StoragePage data={data} />
        ) : route.page === "settings" ? (
          <SettingsPage data={data} />
        ) : project ? (
          <ProjectPage key={project.id} project={project} tab={route.tab} data={data} go={go} />
        ) : (
          <div className="empty">
            <p>This project no longer exists.</p>
            <button className="btn" onClick={() => go({ page: "projects" })}>Back to projects</button>
          </div>
        )}
      </main>
    </div>
  );
}
