import { useEffect, useMemo, useRef, useState } from "react";
import { api, type AvailableUpdate } from "./api";
import { Fixes } from "./fixes";
import { Setup, SettingsView, type UpdateOffer } from "./forms";
import type { ClaudeStatus, Fix, HistoryRecord, Live, Settings } from "./types";
import { CheckButton, Segmented } from "./ui";
import { ago, pollLabel } from "./util";
import { Activity, Detail, History } from "./views";

type View = "activity" | "history" | "detail" | "fixes" | "settings";
type Filter = "all" | "approved" | "rejected" | "failed";

const UPDATE_CHECK_MS = 6 * 60 * 60 * 1000;

/** Offers a newer release; installing is always the user's choice. */
/** Offers a newer release; installing is always the user's choice. */
function UpdateBanner({ offer, onDismiss }: { offer: UpdateOffer; onDismiss: () => void }) {
  if (!offer.update) return null;
  return (
    <div className="banner slim tint-ok" role="status">
      <b className="tone-ok">Warden {offer.update.version} is available</b>
      {/* Deliberately no release notes: the bar only says an update exists. */}
      <span className="grow ellipsis">
        {offer.phase === "installing" ? `Downloading${offer.percent === null ? "…" : ` ${offer.percent}%`}`
          : offer.phase === "error" ? <span className="tone-critical selectable" title={offer.error}>Update failed: {offer.error}</span>
          : offer.reviewing ? "Available once the current review finishes"
          : ""}
      </span>
      {offer.phase !== "installing" && (
        <>
          <button className="btn primary" onClick={offer.install} disabled={offer.reviewing}>{offer.phase === "error" ? "Try again" : "Update and restart"}</button>
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
  const [fixes, setFixes] = useState<Fix[]>([]);
  const [claude, setClaude] = useState<ClaudeStatus | null>(null);
  const [view, setView] = useState<View>("activity");
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const [project, setProject] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [period, setPeriod] = useState<"week" | "all">("week");
  const [now, setNow] = useState(Date.now());
  const [saveError, setSaveError] = useState<string | null>(null);
  const [update, setUpdate] = useState<AvailableUpdate | null>(null);
  const [updateHidden, setUpdateHidden] = useState(false);

  const notifiedVersion = useRef<string | null>(null);
  const notificationsOn = useRef(true);
  notificationsOn.current = settings?.notifications ?? true;

  const [install, setInstall] = useState<{ phase: "idle" | "installing" | "error"; percent: number | null; error: string }>({ phase: "idle", percent: null, error: "" });

  const [lastUpdateCheck, setLastUpdateCheck] = useState<Date | null>(null);

  /** Checks GitHub for a newer release. Rejects if the check itself fails. */
  const checkForUpdate = async () => {
    const found = await api.checkForUpdate();
    setLastUpdateCheck(new Date());
    setUpdate(found);
    if (found) setUpdateHidden(false);
    // Once per version, so the offer is seen even when the window is closed.
    if (found && notificationsOn.current && notifiedVersion.current !== found.version) {
      notifiedVersion.current = found.version;
      api.notify(`Warden ${found.version} is available`, "Open Warden to update.");
    }
    return found;
  };
  // Background checks stay quiet when offline or GitHub is unreachable, and try again later.
  const lookForUpdate = () => {
    checkForUpdate().catch(() => {});
  };
  const startInstall = () => {
    setInstall({ phase: "installing", percent: null, error: "" });
    api.installUpdate((percent) => setInstall({ phase: "installing", percent, error: "" }))
      .catch((e) => setInstall({ phase: "error", percent: null, error: String(e) }));
  };
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
    api.getFixes().then(setFixes);
    const stops = [api.onLive(setLive), api.onHistory(setHistory), api.onSettings(setSettings), api.onFixes(setFixes)];
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

  const offer: UpdateOffer = { update, lastChecked: lastUpdateCheck, ...install, reviewing: live.status === "reviewing", install: startInstall };

  const open = (recordId: string) => {
    setSelected(recordId);
    setView("detail");
  };
  const record = history.find((r) => r.recordId === selected);
  const reviewing = live.status === "reviewing";

  // History is one row per pull request: its latest review, with earlier
  // reviews of the same PR reachable from the detail page.
  const byPr = new Map<number, HistoryRecord[]>();
  for (const r of history) byPr.set(r.pr.id, [...(byPr.get(r.pr.id) ?? []), r]);
  const groups = [...byPr.values()].map((reviews) => ({ latest: reviews[0], count: reviews.length }));
  // The sidebar counts, and the History list by default, cover the last 7 days.
  const weekAgo = now - 7 * 24 * 60 * 60 * 1000;
  const recent = groups.filter((g) => new Date(g.latest.finishedAt).getTime() >= weekAgo);
  let rows = period === "week" ? recent : groups;
  if (filter !== "all") rows = rows.filter((g) => g.latest.status === filter);
  if (project) rows = rows.filter((g) => g.latest.pr.project === project);
  if (search) {
    const q = search.toLowerCase();
    rows = rows.filter(({ latest: r }) => `${r.pr.title} ${r.pr.author} ${r.pr.repo} ${r.pr.project} ${r.pr.id}`.toLowerCase().includes(q));
  }

  const projects = settings.projects.length ? settings.projects : [...new Set(history.map((r) => r.pr.project))].sort();
  const statusText = { setup: "Not set up", paused: "Paused", reviewing: "Reviewing", error: "Problem", idle: "Monitoring" }[live.status];
  const statusColor = live.status === "error" ? "var(--critical)" : live.status === "paused" ? "var(--faint)" : "var(--ok)";
  const titles: Record<View, string> = { activity: "Activity", history: "Review history", detail: "Review", fixes: "Fixes", settings: "Settings" };
  const fixesWaiting = fixes.filter((f) => f.status === "ready").length;
  const fixInProgress = fixes.some((f) => f.status === "generating" || f.status === "pushing");
  const claudeProblem = claude && (!claude.found || !claude.signedIn);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="drag" data-tauri-drag-region />
        <div className="status-card" style={{ "--status": statusColor } as React.CSSProperties}>
          <div className="status-title"><i className={`dot ${live.status === "idle" || reviewing ? "pulse" : ""}`} style={{ background: statusColor }} />{statusText}</div>
          <div className="status-sub mono">
            {settings.projects.length || "All"} projects · {settings.people.length ? `${settings.people.length} people` : "everyone"}<br />
            {live.checking ? "checking now…" : `checked ${ago(live.lastCheck, now)}`} · every {pollLabel(settings.pollSeconds)}
          </div>
        </div>
        <nav className="nav" aria-label="Main">
          <button className={`nav-item ${view === "activity" ? "on" : ""}`} onClick={() => setView("activity")}>
            <svg className="glyph" viewBox="0 0 16 16" aria-hidden><path d="M1.75 8h2.5l1.75-4.5 4 9 1.75-4.5h2.5" /></svg><span className="grow">Activity</span>
            {reviewing && <span className="badge mono">1</span>}
          </button>
          <button className={`nav-item ${view === "history" || view === "detail" ? "on" : ""}`} onClick={() => { setProject(null); setView("history"); }}>
            <svg className="glyph" viewBox="0 0 16 16" aria-hidden><circle cx="8" cy="8" r="6.25" /><path d="M8 4.75V8l2.25 1.5" /></svg><span className="grow">History</span><span className="count mono" title="Pull requests reviewed in the last 7 days">{recent.length}</span>
          </button>
          <button className={`nav-item ${view === "fixes" ? "on" : ""}`} onClick={() => setView("fixes")}>
            <svg className="glyph" viewBox="0 0 16 16" aria-hidden><path d="M9.9 2.2a3.4 3.4 0 0 0-3.6 4.6L2 11.1a1.5 1.5 0 0 0 0 2.1l.8.8a1.5 1.5 0 0 0 2.1 0l4.3-4.3a3.4 3.4 0 0 0 4.6-3.6l-2.1 2.1-1.9-.5-.5-1.9z" /></svg>
            <span className="grow">Fixes</span>
            {fixesWaiting > 0 ? <span className="badge mono" title="Fixes waiting for your review">{fixesWaiting}</span> : fixInProgress ? <span className="spinner" aria-label="A fix is in progress" /> : null}
          </button>
          <button className={`nav-item ${view === "settings" ? "on" : ""}`} onClick={() => setView("settings")}>
            <svg className="glyph" viewBox="0 0 16 16" aria-hidden><circle cx="8" cy="8" r="2" /><path d="M8 1.75v1.5M8 12.75v1.5M1.75 8h1.5M12.75 8h1.5M3.6 3.6l1.05 1.05M11.35 11.35l1.05 1.05M3.6 12.4l1.05-1.05M11.35 4.65l1.05-1.05" /><circle cx="8" cy="8" r="4.25" /></svg><span className="grow">Settings</span>
          </button>
        </nav>
        {projects.length > 0 && <div className="section-label mono">Projects</div>}
        <div className="nav" style={{ overflow: "auto" }}>
          {projects.map((p) => (
            <button key={p} className={`project ${view === "history" && project === p ? "on" : ""}`} onClick={() => { setProject(p); setView("history"); }}>
              <span className="ellipsis">{p}</span><span className="count mono" title="Pull requests reviewed in the last 7 days">{recent.filter((g) => g.latest.pr.project === p).length}</span>
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
              <Segmented label="Period" value={period} onChange={setPeriod}
                options={[{ value: "week", label: "7 days" }, { value: "all", label: "90 days" }]} />
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
        {/* Outside the scrolling area, so it is visible wherever the page is scrolled to. */}
        {!updateHidden && <UpdateBanner offer={offer} onDismiss={() => setUpdateHidden(true)} />}
        <div className="body" ref={bodyRef}>
          {saveError && <div className="banner tint-critical" role="alert"><b className="tone-critical">Settings were not saved</b><span className="grow">{saveError}</span></div>}
          {claudeProblem && view !== "settings" && (
            <div className="banner tint-critical" role="alert">
              <b className="tone-critical">Claude Code {claude.found ? "is not signed in" : "was not found"}</b>
              <span className="grow">{claude.found ? "Reviews will fail until you run `claude auth login` in a terminal." : "Install the Claude Code CLI or set its path in Settings."}</span>
              <button className="btn" onClick={() => api.claudeStatus().then(setClaude)}>Check again</button>
            </div>
          )}
          {view === "activity" && <Activity live={live} history={history} settings={settings} now={now} onOpen={open} onHistory={() => setView("history")} />}
          {view === "history" && <History rows={rows} now={now} onOpen={open}
            emptyHint={period === "week" && groups.length > recent.length ? "No reviews match in the last 7 days. Switch to \"90 days\" to see older ones." : "No reviews match."} />}
          {view === "detail" && (record
            ? <Detail record={record} reviews={byPr.get(record.pr.id) ?? [record]} now={now} onOpen={open}
                pendingFix={fixes.some((f) => f.pr.id === record.pr.id && (f.status === "ready" || f.status === "generating" || f.status === "pushing"))}
                onFix={(issues) => api.startFix(record.recordId, issues).then(() => setView("fixes"))} onShowFixes={() => setView("fixes")} onRetry={() => api.retryReview(record.pr.id).then(() => setView("activity"))} onSettings={() => setView("settings")} />
            : <div className="empty">This review is no longer in the history.</div>)}
          {view === "fixes" && <Fixes fixes={fixes} now={now} />}
          {view === "settings" && <SettingsView settings={settings} change={change} onRunSetup={() => change({ setupComplete: false })} offer={offer} onCheck={checkForUpdate} />}
        </div>
      </main>
    </div>
  );
}
