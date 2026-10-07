import { useEffect, useMemo, useRef, useState } from "react";
import { api, type AvailableUpdate } from "./api";
import { Setup, SettingsView } from "./forms";
import type { ClaudeStatus, HistoryRecord, Live, Settings } from "./types";
import { CheckButton, Segmented } from "./ui";
import { ago, pollLabel } from "./util";
import { Activity, Detail, History } from "./views";

type View = "activity" | "history" | "detail" | "settings";
type Filter = "all" | "approved" | "rejected" | "failed";

const UPDATE_CHECK_MS = 6 * 60 * 60 * 1000;

/** Offers a newer release; installing is always the user's choice. */
function UpdateBanner({ update, reviewing, onDismiss }: { update: AvailableUpdate; reviewing: boolean; onDismiss: () => void }) {
  const [state, setState] = useState<{ phase: "idle" | "installing" | "error"; percent: number | null; error: string }>({ phase: "idle", percent: null, error: "" });
  const install = () => {
    setState({ phase: "installing", percent: null, error: "" });
    api.installUpdate((percent) => setState({ phase: "installing", percent, error: "" }))
      .catch((e) => setState({ phase: "error", percent: null, error: String(e) }));
  };
  return (
    <div className="banner tint-ok" role="status">
      <b className="tone-ok">Warden {update.version} is available</b>
      <span className="grow">
        {state.phase === "installing" ? `Downloading${state.percent === null ? "…" : ` ${state.percent}%`} · Warden will restart when it is ready.`
          : state.phase === "error" ? <span className="tone-critical selectable">Update failed: {state.error}</span>
          : reviewing ? "A review is running. You can update once it has finished."
          : update.notes || "Install it now; Warden restarts and carries on monitoring."}
      </span>
      {state.phase !== "installing" && (
        <>
          <button className="btn primary" onClick={install} disabled={reviewing}>{state.phase === "error" ? "Try again" : "Update and restart"}</button>
          <button className="btn ghost" onClick={onDismiss}>Later</button>
        </>
      )}
    </div>
  );
}

function useSystemDark(): boolean {
  const query = useMemo(() => window.matchMedia("(prefers-color-scheme: dark)"), []);
  const [dark, setDark] = useState(query.matches);
  useEffect(() => {
    const onChange = () => setDark(query.matches);
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, [query]);
  return dark;
}

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [live, setLive] = useState<Live | null>(null);
  const [history, setHistory] = useState<HistoryRecord[]>([]);
  const [claude, setClaude] = useState<ClaudeStatus | null>(null);
  const [view, setView] = useState<View>("activity");
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [project, setProject] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [now, setNow] = useState(Date.now());
  const [saveError, setSaveError] = useState<string | null>(null);
  const [update, setUpdate] = useState<AvailableUpdate | null>(null);
  const [updateHidden, setUpdateHidden] = useState(false);

  const lookForUpdate = () =>
    api.checkForUpdate().then((found) => {
      setUpdate(found);
      if (found) setUpdateHidden(false);
    }, () => {
      // Offline or GitHub unreachable: stay quiet and try again later.
    });
  useEffect(() => {
    lookForUpdate();
    const timer = setInterval(lookForUpdate, UPDATE_CHECK_MS);
    return () => clearInterval(timer);
  }, []);
  const bodyRef = useRef<HTMLDivElement>(null);
  const systemDark = useSystemDark();

  useEffect(() => {
    api.getSettings().then(async (loaded) => {
      // First run: start from the command-line tool's settings if it was used before.
      if (!loaded.setupComplete && !loaded.organization) {
        const legacy = await api.importLegacy();
        if (legacy) loaded = { ...loaded, organization: legacy.organization, projects: legacy.projects, people: legacy.people };
      }
      setSettings(loaded);
    });
    api.getLive().then(setLive);
    api.getHistory().then(setHistory);
    api.claudeStatus().then(setClaude);
    const stops = [api.onLive(setLive), api.onHistory(setHistory), api.onSettings(setSettings)];
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      stops.forEach((stop) => stop());
      clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    bodyRef.current?.scrollTo(0, 0);
  }, [view, selected]);

  const theme = settings?.theme === "light" || settings?.theme === "dark" ? settings.theme : systemDark ? "dark" : "light";
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  if (!settings || !live) return null;

  const change = (patch: Partial<Settings>) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    // Setup is saved in one go at the end, so half-entered values never start monitoring.
    if (next.setupComplete) api.saveSettings(next).then(() => setSaveError(null), (e) => setSaveError(String(e)));
  };

  if (!settings.setupComplete) {
    return (
      <div className="app">
        <Setup settings={settings} change={change} onFinish={() => {
          const next = { ...settings, setupComplete: true, paused: false };
          setSettings(next);
          api.saveSettings(next).then(() => setSaveError(null), (e) => setSaveError(String(e)));
          setView("activity");
        }} />
      </div>
    );
  }

  const open = (recordId: string) => {
    setSelected(recordId);
    setView("detail");
  };
  const record = history.find((r) => r.recordId === selected);
  const reviewing = live.status === "reviewing";

  let rows = history;
  if (filter !== "all") rows = rows.filter((r) => r.status === filter);
  if (project) rows = rows.filter((r) => r.pr.project === project);
  if (search) {
    const q = search.toLowerCase();
    rows = rows.filter((r) => `${r.pr.title} ${r.pr.author} ${r.pr.repo} ${r.pr.project} ${r.pr.id}`.toLowerCase().includes(q));
  }

  const projects = settings.projects.length ? settings.projects : [...new Set(history.map((r) => r.pr.project))].sort();
  const statusText = { setup: "Not set up", paused: "Paused", reviewing: "Reviewing", error: "Problem", idle: "Monitoring" }[live.status];
  const statusColor = live.status === "error" ? "var(--critical)" : live.status === "paused" ? "var(--faint)" : "var(--ok)";
  const titles: Record<View, string> = { activity: "Activity", history: "Review history", detail: "Review", settings: "Settings" };
  const claudeProblem = claude && (!claude.found || !claude.signedIn);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="drag" data-tauri-drag-region />
        <div className="status-card">
          <div className="status-title"><i className={`dot ${live.status === "idle" || reviewing ? "pulse" : ""}`} style={{ background: statusColor }} />{statusText}</div>
          <div className="status-sub mono">
            {settings.projects.length || "All"} projects · {settings.people.length ? `${settings.people.length} people` : "everyone"}<br />
            {live.checking ? "checking now…" : `checked ${ago(live.lastCheck, now)}`} · every {pollLabel(settings.pollSeconds)}
          </div>
        </div>
        <nav className="nav" aria-label="Main">
          <button className={`nav-item ${view === "activity" ? "on" : ""}`} onClick={() => setView("activity")}>
            <span className="glyph" style={{ borderRadius: "50%" }} /><span className="grow">Activity</span>
            {reviewing && <span className="badge mono">1</span>}
          </button>
          <button className={`nav-item ${view === "history" || view === "detail" ? "on" : ""}`} onClick={() => { setProject(null); setView("history"); }}>
            <span className="glyph" style={{ borderRadius: 2 }} /><span className="grow">History</span><span className="count mono">{history.length}</span>
          </button>
          <button className={`nav-item ${view === "settings" ? "on" : ""}`} onClick={() => setView("settings")}>
            <span className="glyph" style={{ transform: "rotate(45deg)" }} /><span className="grow">Settings</span>
          </button>
        </nav>
        {projects.length > 0 && <div className="section-label mono">Projects</div>}
        <div className="nav" style={{ overflow: "auto" }}>
          {projects.map((p) => (
            <button key={p} className={`project ${view === "history" && project === p ? "on" : ""}`} onClick={() => { setProject(p); setView("history"); }}>
              <span className="ellipsis">{p}</span><span className="count mono">{history.filter((r) => r.pr.project === p).length}</span>
            </button>
          ))}
        </div>
        <div className="side-foot mono">
          <div className="inline" style={{ gap: 6 }}><i className="dot" style={{ width: 6, height: 6, background: live.status === "error" ? "var(--critical)" : "var(--ok)" }} />{settings.organization}</div>
          <div className={claudeProblem ? "tone-critical" : ""}>
            {!claude ? "claude …" : !claude.found ? "claude not found" : !claude.signedIn ? "claude not signed in" : `claude ${claude.version.split(" ")[0]}`}
          </div>
        </div>
      </aside>

      <main className="main">
        <header className="topbar" data-tauri-drag-region>
          {view === "detail" && <button className="tone-muted" onClick={() => setView("history")}>‹ History</button>}
          <h1>{titles[view]}</h1>
          <div className="spacer" data-tauri-drag-region />
          {view === "history" && (
            <>
              {project && <button className="chip" onClick={() => setProject(null)} aria-label={`Clear project filter ${project}`}>{project} ×</button>}
              <Segmented label="Filter by decision" value={filter} onChange={setFilter}
                options={[{ value: "all", label: "All" }, { value: "approved", label: "Approved" }, { value: "rejected", label: "Rejected" }, { value: "failed", label: "Failed" }]} />
              <input className="search" placeholder="Search PRs, people, repos" aria-label="Search reviews" value={search} onChange={(e) => setSearch(e.target.value)} />
            </>
          )}
          {view === "activity" && (
            <>
              <CheckButton checking={live.checking} disabled={settings.paused || reviewing} />
              <button className="btn" onClick={() => change({ paused: !settings.paused })}>{settings.paused ? "Resume" : "Pause"}</button>
            </>
          )}
        </header>
        <div className="body" ref={bodyRef}>
          {update && !updateHidden && <UpdateBanner update={update} reviewing={reviewing} onDismiss={() => setUpdateHidden(true)} />}
          {saveError && <div className="banner tint-critical" role="alert"><b className="tone-critical">Settings were not saved</b><span className="grow">{saveError}</span></div>}
          {claudeProblem && view !== "settings" && (
            <div className="banner tint-critical" role="alert">
              <b className="tone-critical">Claude Code {claude.found ? "is not signed in" : "was not found"}</b>
              <span className="grow">{claude.found ? "Reviews will fail until you run `claude auth login` in a terminal." : "Install the Claude Code CLI or set its path in Settings."}</span>
              <button className="btn" onClick={() => api.claudeStatus().then(setClaude)}>Check again</button>
            </div>
          )}
          {view === "activity" && <Activity live={live} history={history} settings={settings} now={now} onOpen={open} onHistory={() => setView("history")} />}
          {view === "history" && <History rows={rows} now={now} onOpen={open} />}
          {view === "detail" && (record
            ? <Detail record={record} onRetry={() => api.retryReview(record.pr.id).then(() => setView("activity"))} onSettings={() => setView("settings")} />
            : <div className="empty">This review is no longer in the history.</div>)}
          {view === "settings" && <SettingsView settings={settings} change={change} onRunSetup={() => change({ setupComplete: false })} onUpdateFound={lookForUpdate} />}
        </div>
      </main>
    </div>
  );
}
