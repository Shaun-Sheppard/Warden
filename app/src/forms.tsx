import { useEffect, useState, type ReactNode } from "react";
import { api, type AvailableUpdate } from "./api";
import type { AreaMapping, AutoFixScope, ClaudeStatus, ProjectConfig, ProjectStructure, Settings, TriageChoices } from "./types";
import { Segmented, SwitchRow, TagInput } from "./ui";
import { avatarColor } from "./util";

type Change = (patch: Partial<Settings>) => void;

type ConnState = { state: "idle" | "testing" | "ok" | "error"; text: string; projects: string[] };

/** Organization and token, with a connection test that also stores a new token. */
export function Connect({ settings, change, onConnected }: {
  settings: Settings; change: Change; onConnected?: (ok: boolean, projects: string[]) => void;
}) {
  const [pat, setPat] = useState("");
  const [stored, setStored] = useState(false);
  const [conn, setConn] = useState<ConnState>({ state: "idle", text: "Not tested", projects: [] });
  useEffect(() => {
    api.hasPat().then(setStored);
  }, []);

  const test = async () => {
    setConn({ state: "testing", text: "Connecting…", projects: [] });
    try {
      const result = await api.testConnection(settings.organization, pat || null);
      const visible = result.projectsNote ? "token cannot list projects, so name them below" : `${result.projects.length} projects visible`;
      setConn({ state: "ok", text: `Connected as ${result.user} · ${visible}`, projects: result.projects });
      if (pat) {
        setStored(true);
        setPat("");
      }
      onConnected?.(true, result.projects);
    } catch (e) {
      setConn({ state: "error", text: String(e), projects: [] });
      onConnected?.(false, []);
    }
  };
  const tone = { idle: "tone-faint", testing: "tone-muted", ok: "tone-ok", error: "tone-critical" }[conn.state];

  return (
    <div className="fields">
      <label className="field"><span>Organisation</span>
        <div className="input-wrap">
          <span className="prefix mono">dev.azure.com/</span>
          <input className="mono" value={settings.organization} spellCheck={false}
            onChange={(e) => { change({ organization: e.target.value.trim() }); setConn({ state: "idle", text: "Not tested", projects: [] }); onConnected?.(false, []); }} />
        </div>
      </label>
      <label className="field"><span>Personal access token</span>
        <input className="input mono" type="password" value={pat} autoComplete="off"
          placeholder={stored ? "Stored in your keychain · paste a new one to replace it" : "Paste your token"}
          onChange={(e) => setPat(e.target.value)} />
        <span className="help">Needs Code (Read &amp; Write) and Work Items (Read), which is used to check acceptance criteria. Add Project and Team (Read) to watch every project without naming them. Kept only in your system keychain.</span>
      </label>
      <div className="inline">
        <button className="btn" onClick={test} disabled={conn.state === "testing" || !settings.organization || (!pat && !stored)}>
          {pat ? "Test and save token" : "Test connection"}
        </button>
        <span className={`mono selectable ${tone}`} style={{ fontSize: 12 }} role="status">{conn.text}</span>
      </div>
    </div>
  );
}

export function Scope({ settings, change, projectChoices, showExisting = false }: {
  settings: Settings; change: Change; projectChoices: string[]; showExisting?: boolean;
}) {
  const [people, setPeople] = useState<string[]>([]);
  const projectsKey = settings.projects.join("|");
  useEffect(() => {
    if (!settings.organization) return;
    api.listPeople(settings.organization, settings.projects).then(setPeople).catch(() => setPeople([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings.organization, projectsKey]);
  const nProj = settings.projects.length;
  const nPeople = settings.people.length;
  return (
    <div className="fields" style={{ gap: 18 }}>
      <div className="field"><span>Projects</span>
        <TagInput id="project-choices" values={settings.projects} onChange={(projects) => change({ projects })} placeholder="Add project, press Enter" suggestions={projectChoices} />
        <span className="help">{nProj ? `Watching ${nProj} project${nProj > 1 ? "s" : ""} in ${settings.organization || "your organisation"}.` : "Empty: every project in the organisation is watched."}</span>
      </div>
      <div className="field"><span>Only review PRs raised by</span>
        <TagInput id="people-choices" values={settings.people} onChange={(p) => change({ people: p })} placeholder="Add person, press Enter" suggestions={people} people />
        <span className="help">
          {nPeople ? `Only PRs raised by ${nPeople > 1 ? `these ${nPeople} people` : "this person"} are reviewed. Others are ignored.` : "Empty: PRs from everyone are reviewed."}
          {" "}Names must match Azure DevOps exactly; pick from the suggestions where you can.
        </span>
      </div>
      {showExisting && (
        <SwitchRow title="Also review pull requests that are already open" on={settings.reviewExisting} onChange={(reviewExisting) => change({ reviewExisting })}
          help="Off: only pull requests opened after you start monitoring are reviewed." />
      )}
    </div>
  );
}

/** The switch that lets Warden vote and merge, behind an explicit confirmation. */
export function ApproveSetting({ settings, change }: { settings: Settings; change: Change }) {
  const [confirming, setConfirming] = useState(false);
  return (
    <>
      <SwitchRow title="Vote, and auto-complete clean PRs" on={settings.approveAndComplete}
        onChange={(on) => (on ? setConfirming(true) : (setConfirming(false), change({ approveAndComplete: false })))}
        help="No critical or major issues: approve and set auto-complete. Otherwise: vote Waiting for author, and review again when new commits are pushed." />
      {confirming && !settings.approveAndComplete && (
        <div className="warning tint-major" role="alertdialog" aria-label="Confirm voting">
          <span><b className="tone-major">Votes are cast as you.</b> Approvals are recorded under your name on the strength of an AI review alone, and auto-complete merges the pull request once its policies pass, without a person reading the code.</span>
          <div className="inline">
            <button className="btn primary" onClick={() => { change({ approveAndComplete: true }); setConfirming(false); }}>Turn on</button>
            <button className="btn ghost" onClick={() => setConfirming(false)}>Cancel</button>
          </div>
        </div>
      )}
    </>
  );
}

export function ClaudeCheck({ status, onRecheck }: { status: ClaudeStatus | null; onRecheck: () => void }) {
  return (
    <div className="terminal mono" role="status">
      {!status ? <div className="tone-muted">Checking…</div> : (
        <>
          <div className="tone-muted">$ which claude</div>
          <div className={status.found ? "selectable" : "tone-critical"}>{status.found ? status.path : "not found"}</div>
          {status.found && (
            <>
              <div className="tone-muted">$ claude --version</div>
              <div className={status.signedIn && status.supported ? "tone-ok" : "tone-critical"}>
                {status.version || "unknown"} · {!status.supported ? "too old, needs updating" : status.signedIn ? "signed in" : "not signed in"}
              </div>
            </>
          )}
          {!status.gitFound && <div className="tone-critical">git was not found</div>}
        </>
      )}
      {status && (!status.found || !status.supported || !status.signedIn || !status.gitFound) && (
        <div style={{ marginTop: 8, fontFamily: "inherit" }}>
          <span className="tone-muted">
            {!status.found ? "Install the Claude Code CLI, or set its path in Settings."
              : !status.supported ? "Run `claude update` in a terminal. Warden needs a version that can run reviews safely confined."
              : !status.signedIn ? "Run `claude auth login` in a terminal." : "Install git."}{" "}
          </span>
          <button className="link" onClick={onRecheck}>Check again</button>
        </div>
      )}
    </div>
  );
}

export function useClaudeStatus(dependency: string): [ClaudeStatus | null, () => void] {
  const [status, setStatus] = useState<ClaudeStatus | null>(null);
  const check = () => {
    setStatus(null);
    api.claudeStatus().then(setStatus);
  };
  useEffect(check, [dependency]);
  return [status, check];
}

/** Editor for Claude's guidance, starting from the built-in text. Empty in settings means "use the built-in". */
function PromptSetting({ value, onSave, loadDefault, help }: {
  value: string; onSave: (value: string) => void; loadDefault: () => Promise<string>; help: ReactNode;
}) {
  const [builtIn, setBuiltIn] = useState("");
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    loadDefault().then((found) => {
      setBuiltIn(found);
      setText((current) => current ?? (value.trim() ? value : found));
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  if (text === null) return null;
  const custom = value.trim() !== "";
  const dirty = text.trim() !== (custom ? value.trim() : builtIn.trim());
  // Saving text identical to the built-in guidance keeps "default", so future improvements to it still apply.
  const save = () => onSave(text.trim() === builtIn.trim() ? "" : text.trim());
  return (
    <div className="fields">
      <label className="field"><span>Instructions for Claude</span>
        <textarea className="input mono prompt" value={text} spellCheck={false} onChange={(e) => setText(e.target.value)} />
      </label>
      <span className="help">{help}</span>
      <div className="inline">
        <button className="btn primary" disabled={!dirty || !text.trim()} onClick={save}>Save prompt</button>
        <button className="btn" disabled={!custom && !dirty} onClick={() => { setText(builtIn); onSave(""); }}>Reset to default</button>
        <span className="help" role="status">{dirty ? "Unsaved changes" : custom ? "Using your custom prompt" : "Using the built-in prompt"}</span>
      </div>
    </div>
  );
}

/** Version, and a manual update check. */
/** A newer release, and where its installation has got to. Shared by the bar at the top and Settings → About. */
export interface UpdateOffer {
  update: AvailableUpdate | null;
  /** When GitHub was last asked successfully, by the app or by the user. */
  lastChecked: Date | null;
  phase: "idle" | "installing" | "error";
  /** What the installation is doing right now, in words. */
  progress: string;
  error: string;
  /** A review or fix is in flight; the update cannot be started until it finishes. */
  reviewing: boolean;
  install: () => void;
}

/** Version, a manual update check, and the update itself when there is one. */
function About({ offer, onCheck }: { offer: UpdateOffer; onCheck: () => Promise<AvailableUpdate | null> }) {
  const [version, setVersion] = useState("");
  const [status, setStatus] = useState<{ text: string; tone: string }>({ text: "", tone: "tone-faint" });
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.appVersion().then(setVersion);
  }, []);
  const check = async () => {
    setBusy(true);
    setStatus({ text: "Checking…", tone: "tone-muted" });
    try {
      await onCheck();
      setStatus({ text: "", tone: "tone-faint" });
    } catch (e) {
      setStatus({ text: `Could not check: ${e}`, tone: "tone-critical" });
    }
    setBusy(false);
  };
  return (
    <div className="fields">
      <span className="mono selectable">Warden {version}</span>
      {offer.update && (
        <div className="warning tint-ok" role="status">
          <b className="tone-ok">Warden {offer.update.version} is available</b>
          {offer.phase === "installing" ? (
            <span><span className="spinner" aria-hidden /> {offer.progress} Warden restarts when it is done.</span>
          ) : (
            <div className="inline" style={{ flexWrap: "wrap" }}>
              <button className="btn solid" onClick={offer.install} disabled={offer.reviewing}>{offer.phase === "error" ? "Try again" : "Update and restart"}</button>
              {offer.reviewing && <span className="help">Available once the review or fix in progress finishes</span>}
            </div>
          )}
          {offer.phase === "error" && <span className="tone-critical selectable" role="alert">Update failed: {offer.error}</span>}
        </div>
      )}
      <div className="inline">
        <button className="btn" onClick={check} disabled={busy || offer.phase === "installing"}>Check for updates</button>
        <span className={`help ${status.text ? status.tone : offer.update ? "tone-faint" : "tone-ok"}`} role="status">
          {status.text || (offer.lastChecked && !offer.update ? "You have the latest version" : "")}
        </span>
      </div>
      {offer.lastChecked && (
        <span className="help">Last checked {offer.lastChecked.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</span>
      )}
      <span className="help">Warden checks GitHub for new releases when it starts and every few hours, and asks before installing one.</span>
    </div>
  );
}

/** "Project\\Area\\Sub" as "Project › Area › Sub". */
const areaLabel = (path: string) => path.split("\\").join(" › ");

/** Whether a mapping is complete, and whether its fix setup can run. */
function mappingStatus(m: AreaMapping): { label: string; tone: "ok" | "major" } {
  if (!m.areaPath || !m.repo || !m.branch) return { label: "Incomplete", tone: "major" };
  const copies = m.copyLocalOnly || (m.copyPaths ?? []).length > 0;
  if (copies && !(m.localPath ?? "").trim()) return { label: "Needs local copy", tone: "major" };
  return { label: "Ready", tone: "ok" };
}

/** One area mapping: a summary row that opens into where it matches and how a fix gets ready. */
function MappingCard({ m, open, onToggle, onChange, onRemove, structure }: {
  m: AreaMapping; open: boolean; onToggle: () => void; onChange: (patch: Partial<AreaMapping>) => void; onRemove: () => void;
  structure: ProjectStructure | "loading" | string | undefined;
}) {
  const [paths, setPaths] = useState((m.copyPaths ?? []).join("\n"));
  const loaded = typeof structure === "object" ? structure : null;
  const status = mappingStatus(m);
  const extra = (m.copyPaths ?? []).length;
  const summary = [
    m.copyLocalOnly ? "Copies untracked files" : "No untracked files",
    extra ? `${extra} extra file${extra === 1 ? "" : "s"}` : null,
    (m.setupCommand ?? "").trim() ? "runs setup command" : "no setup command",
  ].filter(Boolean).join(" · ");
  const repo = m.repo || "the repository";

  return (
    <div className={`map-card ${open ? "open" : ""}`}>
      <div className="map-head">
        <button className="map-toggle" onClick={onToggle} aria-expanded={open}>
          <span className={`chev ${open ? "open" : ""}`}>›</span>
          <span className="stack" style={{ gap: 4, minWidth: 0 }}>
            <span className="map-title">
              <b>{m.areaPath ? areaLabel(m.areaPath) : "Choose an area"}</b>
              <span className="tone-faint">→</span>
              <b>{m.repo || "Choose a repository"}</b>
              {m.branch && <span className="mono map-branch">{m.branch}</span>}
            </span>
            <span className="help">{summary}</span>
          </span>
        </button>
        <span className={`tpill tpill-${status.tone}`}>{status.label}</span>
        <button className="map-remove" aria-label="Remove area" title="Remove area" onClick={onRemove}>×</button>
      </div>

      {open && (
        <>
          <div className="map-section">
            <span className="eyebrow">Match</span>
            <div className="map-grid">
              <label className="field"><span>Area path</span>
                <select className="input" value={m.areaPath} disabled={!loaded} onChange={(e) => onChange({ areaPath: e.target.value })}>
                  <option value="">{structure === "loading" ? "Loading…" : loaded ? "Choose an area" : "—"}</option>
                  {loaded?.areas.map((a) => <option key={a} value={a}>{areaLabel(a)}</option>)}
                </select>
              </label>
              <label className="field"><span>Repository</span>
                <select className="input" value={m.repo} disabled={!loaded}
                  onChange={(e) => onChange({ repo: e.target.value, branch: loaded?.repos.find((r) => r.name === e.target.value)?.defaultBranch || m.branch })}>
                  <option value="">{loaded ? "Choose a repository" : "—"}</option>
                  {loaded?.repos.map((r) => <option key={r.name} value={r.name}>{r.name}</option>)}
                </select>
              </label>
              <label className="field"><span>Base branch</span>
                <input className="input mono" value={m.branch} placeholder="dev" onChange={(e) => onChange({ branch: e.target.value.trim() })} />
              </label>
            </div>
            <span className="help">Pull requests are opened against the base branch.</span>
            {typeof structure === "string" && structure !== "loading" && <span className="help tone-critical">{structure}</span>}
          </div>

          <div className="map-section">
            <div className="stack" style={{ gap: 4 }}>
              <span className="eyebrow">Fix setup</span>
              <span className="help">Each fix starts from a fresh copy of {repo}. These steps get it ready to build and test.</span>
            </div>

            <div className="map-step">
              <span className="step-num">1</span>
              <label className="field">
                <span className="step-title">Your local copy of {repo}</span>
                <input className="input mono" value={m.localPath ?? ""} placeholder={`~/projects/${m.repo || "repository"}`} onChange={(e) => onChange({ localPath: e.target.value })} />
                <span className="help">Only read, never changed. Files are copied from here in step 2.</span>
              </label>
            </div>

            <div className="map-step">
              <span className="step-num">2</span>
              <div className="fields" style={{ gap: 12 }}>
                <span className="step-title">Files to bring across</span>
                <div className="switch-tile" role="button" tabIndex={0}
                  onClick={() => onChange({ copyLocalOnly: !m.copyLocalOnly })}
                  onKeyDown={(e) => { if (e.key === " " || e.key === "Enter") { e.preventDefault(); onChange({ copyLocalOnly: !m.copyLocalOnly }); } }}>
                  <span className="stack" style={{ gap: 2 }}>
                    <span style={{ fontWeight: 500 }}>All untracked files</span>
                    <span className="help">.env, local settings and anything else git ignores. Skips node_modules, bin, obj, dist and .angular, which step 3 rebuilds.</span>
                  </span>
                  <span className={`switch ${m.copyLocalOnly ? "on" : ""}`} role="switch" aria-checked={!!m.copyLocalOnly} aria-label="All untracked files" />
                </div>
                <label className="field">
                  <span>Also copy these tracked files <span className="tone-faint" style={{ fontWeight: 400 }}>· optional</span></span>
                  <textarea className="input mono" rows={2} value={paths} spellCheck={false}
                    placeholder={"Acme.PortalApi/appsettings.Development.json\nAcme.Portal/src/environments/environment.ts"}
                    onChange={(e) => setPaths(e.target.value)}
                    onBlur={() => onChange({ copyPaths: paths.split("\n").map((p) => p.trim()).filter(Boolean) })} />
                  <span className="help">One path per line, relative to the repository. Your local version replaces the committed one for building and testing, and is never committed.</span>
                </label>
              </div>
            </div>

            <div className="map-step">
              <span className="step-num">3</span>
              <label className="field">
                <span className="step-title">Setup command <span className="tone-faint" style={{ fontWeight: 400 }}>· optional</span></span>
                <input className="input mono" value={m.setupCommand ?? ""} placeholder="dotnet restore && cd Acme.Portal && npm ci" onChange={(e) => onChange({ setupCommand: e.target.value })} />
                <span className="help">Runs once before Claude starts. Leave blank and Claude restores packages itself. Nothing it installs or changes is committed.</span>
              </label>
            </div>
          </div>
        </>
      )}
    </div>
  );
}

/** A project's area paths and the repository each one's work belongs to. */
function AreaMappings({ project, areas, onChange }: { project: string; areas: AreaMapping[]; onChange: (areas: AreaMapping[]) => void }) {
  const [structure, setStructure] = useState<ProjectStructure | "loading" | string | undefined>(undefined);
  const load = () => {
    if (!project || structure) return;
    setStructure("loading");
    api.projectStructure(project).then(setStructure, (e) => setStructure(`Could not load areas and repositories: ${e}`));
  };
  // Open the ones that still need something; the rest stay as one-line summaries.
  const [opened, setOpened] = useState<Set<number>>(() => new Set(areas.flatMap((m, i) => (mappingStatus(m).tone === "ok" ? [] : [i]))));
  useEffect(() => {
    if (opened.size > 0) load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opened.size, project]);
  const set = (index: number, patch: Partial<AreaMapping>) => onChange(areas.map((m, i) => (i === index ? { ...m, ...patch } : m)));

  return (
    <div className="map-list">
      {areas.map((m, i) => (
        <MappingCard key={i} m={m} open={opened.has(i)} structure={structure}
          onToggle={() => setOpened((o) => { const next = new Set(o); if (!next.delete(i)) next.add(i); return next; })}
          onChange={(patch) => set(i, patch)}
          onRemove={() => {
            onChange(areas.filter((_, j) => j !== i));
            setOpened((o) => new Set([...o].filter((j) => j !== i).map((j) => (j > i ? j - 1 : j))));
          }} />
      ))}
      <div className="map-fallback">
        <span />
        <span className="map-title"><b>Any other area</b><span className="tone-faint">→</span><span>Left for a person</span></span>
        <span className="help">Always</span>
      </div>
      <div>
        <button className="btn add-area" onClick={() => {
          onChange([...areas, { project, areaPath: "", repo: "", branch: "", localPath: "", copyPaths: [], copyLocalOnly: true, setupCommand: "" }]);
          setOpened(new Set([areas.length]));
        }}>+ Add area</button>
      </div>
    </div>
  );
}

const MERGE_OPTIONS = [{ value: "squash", label: "Squash" }, { value: "noFastForward", label: "Merge" }, { value: "rebase", label: "Rebase" }];

/** Voting and auto-complete for one project, behind an explicit confirmation. */
function ProjectApprove({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  const [confirming, setConfirming] = useState(false);
  return (
    <>
      <SwitchRow title="Vote, and auto-complete clean PRs" on={on}
        onChange={(next) => (next ? setConfirming(true) : (setConfirming(false), onChange(false)))}
        help="No critical or major issues: approve and set auto-complete. Otherwise: vote Waiting for author, and review again when new commits are pushed." />
      {confirming && !on && (
        <div className="warning tint-major" role="alertdialog" aria-label="Confirm voting">
          <span><b className="tone-major">Votes are cast as you.</b> Approvals are recorded under your name on the strength of an AI review alone, and auto-complete merges the pull request once its policies pass, without a person reading the code.</span>
          <div className="inline">
            <button className="btn primary" onClick={() => { onChange(true); setConfirming(false); }}>Turn on</button>
            <button className="btn ghost" onClick={() => setConfirming(false)}>Cancel</button>
          </div>
        </div>
      )}
    </>
  );
}

/** A short summary of what Warden does for a project. */
function projectSummary(c: ProjectConfig, triageOn: boolean) {
  const review = c.review ? `Reviews ${c.people.length ? `PRs by ${c.people.length} ${c.people.length === 1 ? "person" : "people"}` : "every PR"}${c.approveAndComplete ? ", votes and completes" : ""}` : "PR review off";
  const fixes = { none: "", quick: " and fixes quick items", standard: " and fixes quick and standard items" }[c.autoFixScope ?? "none"];
  const triage = c.triage ? `triages${fixes}, ${c.areas.length} area${c.areas.length === 1 ? "" : "s"}` : "triage off";
  return triageOn ? `${review} · ${triage}` : review;
}

/** Everything for one project in one place: its pull request reviews, its board triage and its areas. */
function ProjectCard({ config: c, organization, open, triageOn, onToggle, onChange, onRemove }: {
  config: ProjectConfig; organization: string; open: boolean; triageOn: boolean; onToggle: () => void;
  onChange: (patch: Partial<ProjectConfig>) => void; onRemove: () => void;
}) {
  const [people, setPeople] = useState<string[]>([]);
  const [choices, setChoices] = useState<TriageChoices | null>(null);
  const [removing, setRemoving] = useState(false);
  useEffect(() => {
    if (!open || !organization) return;
    let live = true;
    api.listPeople(organization, [c.name]).then((p) => live && setPeople(p), () => {});
    api.triageChoices([c.name]).then((found) => live && setChoices(found), () => {});
    return () => { live = false; };
  }, [open, organization, c.name]);
  const incomplete = c.triage && c.areas.some((m) => mappingStatus(m).tone !== "ok");

  return (
    <div className={`project-card ${open ? "open" : ""}`}>
      <div className="map-head">
        <button className="map-toggle" onClick={onToggle} aria-expanded={open}>
          <span className={`chev ${open ? "open" : ""}`}>›</span>
          <span className="inline" style={{ gap: 12, minWidth: 0 }}>
          <span className="project-initial" style={{ width: 32, height: 32, borderRadius: 10, fontSize: 14, background: `color-mix(in oklab, oklch(0.6 0.15 ${avatarColor(c.name)}) 16%, var(--panel))`, color: `oklch(0.58 0.16 ${avatarColor(c.name)})` }} aria-hidden>{c.name.trim()[0]?.toUpperCase()}</span>
          <span className="stack" style={{ gap: 4, minWidth: 0 }}>
            <span className="map-title"><b style={{ fontSize: 15 }}>{c.name}</b></span>
            <span className="help">{projectSummary(c, triageOn)}</span>
          </span>
          </span>
        </button>
        <span className="pills">
          {c.review && <span className="tpill tpill-purple">PR review</span>}
          {c.triage && triageOn && <span className={`tpill ${incomplete ? "tpill-major" : "tpill-ok"}`}>{incomplete ? "Triage · finish setup" : "Triage"}</span>}
          {!c.review && !(c.triage && triageOn) && <span className="tpill tpill-neutral">Off</span>}
        </span>
        <button className="map-remove" aria-label={`Remove ${c.name}`} title="Remove project" onClick={() => setRemoving(true)}>×</button>
      </div>
      {removing && (
        <div className="map-section">
          <div className="warning tint-major" role="alertdialog">
            <span>Remove <b>{c.name}</b>? Its pull requests stop being reviewed, its board stops being triaged, and its areas are forgotten.</span>
            <div className="inline">
              <button className="btn primary" onClick={onRemove}>Remove project</button>
              <button className="btn ghost" onClick={() => setRemoving(false)}>Cancel</button>
            </div>
          </div>
        </div>
      )}

      {open && (
        <>
          <div className="map-section">
            <span className="eyebrow">Pull request review</span>
            <SwitchRow title="Review pull requests" on={c.review} onChange={(review) => onChange({ review })}
              help="Claude reviews each new pull request in this project and posts what it finds." />
            {c.review && (
              <>
                <div className="field"><span>Only review PRs raised by</span>
                  <TagInput id={`people-${c.name}`} values={c.people} onChange={(p) => onChange({ people: p })} placeholder="Add person, press Enter" suggestions={people} people />
                  <span className="help">{c.people.length ? "Others' pull requests are ignored." : "Empty: pull requests from everyone are reviewed."} Names must match Azure DevOps exactly.</span>
                </div>
                <ProjectApprove on={c.approveAndComplete} onChange={(approveAndComplete) => onChange({ approveAndComplete })} />
                <div className={`switch-row ${c.approveAndComplete ? "" : "dimmed"}`}><span className="label">Merge type</span>
                  <Segmented label="Merge type" value={c.mergeStrategy} onChange={(mergeStrategy) => onChange({ mergeStrategy })} options={MERGE_OPTIONS} />
                </div>
                <SwitchRow className={c.approveAndComplete ? "" : "dimmed"} title="Delete source branch after merge" on={c.deleteSourceBranch} onChange={(deleteSourceBranch) => onChange({ deleteSourceBranch })} />
              </>
            )}
          </div>

          {triageOn && <div className="map-section">
            <span className="eyebrow">Board triage</span>
            <SwitchRow title="Triage my work items" on={c.triage} onChange={(triage) => onChange({ triage })}
              help="Each check, Warden reads the work items assigned to you here in a to-do state and rates their risk and whether an AI agent could fix them." />
            {c.triage && (
              <>
                <div className="switch-row">
                  <div className="stack">
                    <span className="label">Fix automatically</span>
                    <span className="help">Which items Claude fixes without asking. It fixes, adds tests and runs the build and tests; only if they pass does Warden push and open a pull request. Complex items are never fixed automatically; “Fix with AI” on the Triage page fixes any quick or standard item.</span>
                  </div>
                  <select className="input model-select" style={{ width: 220 }} value={c.autoFixScope ?? "none"} aria-label={`Which items are fixed automatically in ${c.name}`}
                    onChange={(e) => onChange({ autoFixScope: e.target.value as AutoFixScope })}>
                    <option value="none">None</option>
                    <option value="quick">Quick fixes only</option>
                    <option value="standard">Quick and standard fixes</option>
                  </select>
                </div>
                <SwitchRow title="Current sprint only" on={c.currentSprint} onChange={(currentSprint) => onChange({ currentSprint })}
                  help="Only items in the project's current sprint. Items in later sprints or the backlog wait until their sprint starts." />
                <SwitchRow title="Tag and comment on items" on={c.postResults} onChange={(postResults) => onChange({ postResults })}
                  help="Tag each triaged item warden-ai-fixable or warden-needs-human, and comment on the ones for a person, mentioning you. Off keeps triage results in Warden." />
                <div className="map-grid">
                  <div className="field"><span>States that count as “to do”</span>
                    <TagInput id={`states-${c.name}`} values={c.states} onChange={(states) => onChange({ states })} placeholder="Add state, press Enter"
                      suggestions={choices?.states ?? ["To Do", "New", "Proposed", "Approved", "Committed", "Active"]} />
                  </div>
                  <div className="field"><span>Work item types</span>
                    <TagInput id={`types-${c.name}`} values={c.types} onChange={(types) => onChange({ types })} placeholder="Add type, press Enter"
                      suggestions={choices?.types ?? ["Bug", "Product Backlog Item", "User Story", "Task", "Issue"]} />
                  </div>
                </div>
                <div className="stack" style={{ gap: 4, marginTop: 6 }}>
                  <b>Area paths and repositories</b>
                  <span className="help">Which repository Claude fixes each board area in. An area includes everything below it, and the most specific match wins.</span>
                </div>
                <AreaMappings project={c.name} areas={c.areas} onChange={(areas) => onChange({ areas })} />
              </>
            )}
          </div>}
        </>
      )}
    </div>
  );
}

/** Every project Warden works with, each configured on its own. */
function ProjectsSettings({ settings, change, triageOn }: { settings: Settings; change: Change; triageOn: boolean }) {
  const configs = settings.projectConfigs ?? [];
  const [available, setAvailable] = useState<string[]>([]);
  const [adding, setAdding] = useState("");
  const [opened, setOpened] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (settings.organization) api.listProjects().then(setAvailable, () => setAvailable([]));
  }, [settings.organization]);
  const set = (name: string, patch: Partial<ProjectConfig>) =>
    change({ projectConfigs: configs.map((c) => (c.name === name ? { ...c, ...patch } : c)) });
  const choices = available.filter((p) => !configs.some((c) => c.name.toLowerCase() === p.toLowerCase()));
  const add = (name: string) => {
    const trimmed = name.trim();
    if (!trimmed || configs.some((c) => c.name.toLowerCase() === trimmed.toLowerCase())) return;
    const exact = available.find((p) => p.toLowerCase() === trimmed.toLowerCase()) ?? trimmed;
    change({
      projectConfigs: [...configs, {
        name: exact, review: true, people: [], approveAndComplete: false, mergeStrategy: "noFastForward", deleteSourceBranch: false,
        triage: false, autoFix: false, autoFixScope: "none", currentSprint: true, postResults: false, states: ["To Do", "New"], types: ["Bug"], areas: [],
      }],
    });
    setOpened(new Set([exact]));
    setAdding("");
  };

  return (
    <div className="map-list">
      {configs.length === 0 && <div className="note-box"><b>No projects yet</b><p>Add a project to review its pull requests or triage its board.</p></div>}
      {configs.map((c) => (
        <ProjectCard key={c.name} config={c} organization={settings.organization} open={opened.has(c.name)} triageOn={triageOn}
          onToggle={() => setOpened((o) => { const next = new Set(o); if (!next.delete(c.name)) next.add(c.name); return next; })}
          onChange={(patch) => set(c.name, patch)}
          onRemove={() => change({ projectConfigs: configs.filter((x) => x.name !== c.name) })} />
      ))}
      <div className="inline add-project">
        <input className="input" list="project-options" value={adding} placeholder="Project name" aria-label="Project to add"
          onChange={(e) => setAdding(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") add(adding); }} />
        <datalist id="project-options">{choices.map((p) => <option key={p} value={p} />)}</datalist>
        <button className="btn add-area" disabled={!adding.trim()} onClick={() => add(adding)}>+ Add project</button>
      </div>
      <div className="note-box">
        <b>How fixes are checked</b>
        <p>Claude finds and runs each repository's build and tests. Only build and test commands such as dotnet test and npm test are allowed, and nothing is pushed unless they pass. The setup command and tests run on this computer, as you.</p>
      </div>
    </div>
  );
}

/** The models offered for each kind of work; "" is whatever Claude Code itself uses. */
const MODELS = [
  { value: "", label: "Default" }, { value: "fable", label: "Fable" }, { value: "opus", label: "Opus" },
  { value: "sonnet", label: "Sonnet" }, { value: "haiku", label: "Haiku" },
];

const EFFORTS = [
  { value: "", label: "Default effort" }, { value: "low", label: "Low" }, { value: "medium", label: "Medium" },
  { value: "high", label: "High" }, { value: "xhigh", label: "Extra high" }, { value: "max", label: "Max" },
];

function ModelSetting({ title, help, value, onChange, effort, onEffort }: {
  title: string; help: string; value: string; onChange: (value: string) => void; effort: string; onEffort: (effort: string) => void;
}) {
  const known = MODELS.some((m) => m.value === value);
  return (
    <div className="switch-row model-row">
      <span className="stack" style={{ gap: 2 }}><span className="label">{title}</span><span className="help">{help}</span></span>
      <span className="model-pickers">
        <select className="input model-select" value={value} onChange={(e) => onChange(e.target.value)} aria-label={`${title}: model`}>
          {!known && <option value={value}>{value}</option>}
          {MODELS.map((m) => <option key={m.value} value={m.value}>{m.label}</option>)}
        </select>
        <select className="input model-select" value={effort} onChange={(e) => onEffort(e.target.value)} aria-label={`${title}: effort`}>
          {EFFORTS.map((m) => <option key={m.value} value={m.value}>{m.label}</option>)}
        </select>
      </span>
    </div>
  );
}

type SettingsTab = "general" | "review" | "triage" | "projects";
const SETTINGS_TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "General" }, { key: "review", label: "PR review" }, { key: "triage", label: "Triage" }, { key: "projects", label: "Projects" },
];

export function SettingsView({ settings, change, onRunSetup, offer, onCheck }: {
  settings: Settings; change: Change; onRunSetup: () => void; offer: UpdateOffer; onCheck: () => Promise<AvailableUpdate | null>;
}) {
  const [claude, recheck] = useClaudeStatus(settings.cliPath);
  const triageOn = settings.triageOn ?? (settings.projectConfigs ?? []).some((c) => c.triage);
  const [tab, setTabState] = useState<SettingsTab>(() => {
    try { return (localStorage.getItem("warden.settings.tab") as SettingsTab) || "general"; } catch { return "general"; }
  });
  const setTab = (t: SettingsTab) => { setTabState(t); try { localStorage.setItem("warden.settings.tab", t); } catch { /* not kept */ } };

  return (
    <div className="settings">
      <div className="seg settings-tabs" role="tablist" aria-label="Settings sections">
        {SETTINGS_TABS.map((t) => (
          <button key={t.key} role="tab" aria-selected={tab === t.key} className={tab === t.key ? "on" : ""} onClick={() => setTab(t.key)}>{t.label}</button>
        ))}
      </div>

      {tab === "general" && (
        <>
          <section className="setting-group">
            <div><h2>Azure DevOps</h2><span className="help">Your organisation and the token Warden uses.</span></div>
            <Connect settings={settings} change={change} />
          </section>
          <section className="setting-group">
            <div><h2>Checking</h2><span className="help">How often Warden looks for new pull requests and board items, and how it tells you about them.</span></div>
            <div className="fields">
              <div className="switch-row"><span className="label">Check every</span>
                <Segmented label="Check interval" value={settings.pollSeconds} onChange={(pollSeconds) => change({ pollSeconds })}
                  options={[{ value: 30, label: "30s" }, { value: 60, label: "1m" }, { value: 300, label: "5m" }, { value: 900, label: "15m" }]} />
              </div>
              <SwitchRow title="Notifications" on={settings.notifications} onChange={(notifications) => change({ notifications })}
                help="Show a system notification when a review starts and finishes, when fixes finish or pull requests are approved, and when an update is available." />
              <SwitchRow title="Keep the computer awake" on={!!settings.keepAwake} onChange={(keepAwake) => change({ keepAwake })}
                help="Stops the computer going to sleep when idle while Warden is running, so reviews, triage and fixes carry on while you are away. The screen can still turn off and lock; closing a laptop's lid still puts it to sleep." />
            </div>
          </section>
          <section className="setting-group">
            <div><h2>Claude Code</h2><span className="help">The CLI that runs reviews, triage and fixes.</span></div>
            <div className="fields">
              <label className="field"><span>CLI path</span>
                <input className="input mono" defaultValue={settings.cliPath} placeholder="Found automatically" spellCheck={false}
                  onBlur={(e) => e.target.value.trim() !== settings.cliPath && change({ cliPath: e.target.value.trim() })} />
              </label>
              <ClaudeCheck status={claude} onRecheck={recheck} />
            </div>
          </section>
          <section className="setting-group">
            <div><h2>Appearance</h2></div>
            <div className="fields" style={{ alignItems: "flex-start" }}>
              <Segmented label="Theme" value={settings.theme} onChange={(theme) => change({ theme })}
                options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} />
              <button className="link" style={{ fontSize: 12 }} onClick={onRunSetup}>Run first-time setup again</button>
            </div>
          </section>
          <section className="setting-group">
            <div><h2>About</h2><span className="help">Version and updates.</span></div>
            <About offer={offer} onCheck={onCheck} />
          </section>
        </>
      )}

      {tab === "review" && (
        <>
          <section className="setting-group">
            <div><h2>Pull request review</h2><span className="help">For every project. Which projects are reviewed, whose pull requests, and whether Warden votes are set per project under Projects.</span></div>
            <div className="fields">
              <SwitchRow title="Dry run" on={settings.dryRun} onChange={(dryRun) => change({ dryRun })}
                help="Review pull requests but never post, vote or complete. Use it to see what Warden would do." />
              <ModelSetting title="Model" value={settings.reviewModel ?? ""} onChange={(reviewModel) => change({ reviewModel })}
                effort={settings.reviewEffort ?? ""} onEffort={(reviewEffort) => change({ reviewEffort })}
                help="Reviews run once per push and catch security problems, so they are worth the strongest model." />
              <SwitchRow title="Tag the author in the comment" on={settings.mentionAuthor} onChange={(mentionAuthor) => change({ mentionAuthor })} />
              <SwitchRow title="Don't review pull requests again once approved" on={!!settings.skipApproved} onChange={(skipApproved) => change({ skipApproved })}
                help="When new commits are pushed to a pull request that is already approved (by Warden, by Approve anyway, or with your approval vote), it is not reviewed again. Useful when a build fails after approval and is fixed. Pull requests that were rejected are still reviewed when fixed." />
              <SwitchRow title="Also review pull requests that were already open" on={settings.reviewExisting} onChange={(reviewExisting) => change({ reviewExisting })}
                help="Off: only pull requests opened after monitoring started are reviewed." />
            </div>
          </section>
          <section className="setting-group">
            <div><h2>Review prompt</h2><span className="help">What Claude is asked to check in each pull request.</span></div>
            <PromptSetting value={settings.reviewPrompt} onSave={(reviewPrompt) => change({ reviewPrompt })} loadDefault={api.defaultPrompt}
              help={<>Warden always adds the pull request's details, its linked work items and acceptance criteria, the repo's REVIEW.md, and the answer format, so you only need to say what to look for.
                Keep the Critical / Major / Minor wording: approval depends on those levels.</>} />
          </section>
        </>
      )}

      {tab === "triage" && (
        <section className="setting-group">
          <div><h2>Triage</h2><span className="help">Rate the work items assigned to you, and fix the easy ones with Claude.</span></div>
          <div className="fields">
            <SwitchRow title="Triage my boards" on={triageOn} onChange={(on) => change({ triageOn: on })}
              help={triageOn
                ? "Each check, Warden reads the work items assigned to you in the projects set to triage under Projects, rates them, and can fix them. Off hides triage everywhere and stops it; what was triaged is kept."
                : "Off: Warden does not read your boards, and triage is hidden from the sidebar and Projects. Turn it on to choose which projects are triaged and how items are fixed."} />
          </div>
        </section>
      )}

      {tab === "triage" && triageOn && (
        <>
          <section className="setting-group">
            <div><h2>Board triage</h2><span className="help">For every project. Which projects are triaged, which items count and where their code lives are set per project under Projects.</span></div>
            <div className="fields">
              <div className="switch-row"><span className="label">Fixes at a time</span>
                <Segmented label="Fixes at a time" value={settings.triageMaxFixes ?? 1} onChange={(triageMaxFixes) => change({ triageMaxFixes })}
                  options={[1, 2, 3, 4, 5].map((n) => ({ value: n, label: String(n) }))} />
              </div>
              <span className="help" style={{ marginTop: -8 }}>Across all projects. Each fix builds and tests its own copy of the repository, so more at once uses more of this computer, and test suites that share a database or port can get in each other's way.</span>
              <ModelSetting title="Triage model" value={settings.triageModel ?? ""} onChange={(triageModel) => change({ triageModel })}
                effort={settings.triageEffort ?? ""} onEffort={(triageEffort) => change({ triageEffort })}
                help="Rates every item on your boards, so a lighter model saves the most here." />
              <ModelSetting title="Model for quick fixes" value={settings.fixModelSimple ?? ""} onChange={(fixModelSimple) => change({ fixModelSimple })}
                effort={settings.fixEffortSimple ?? ""} onEffort={(fixEffortSimple) => change({ fixEffortSimple })}
                help="Trivial changes with an obvious fix: a typo, a label, a missing check." />
              <ModelSetting title="Model for standard fixes" value={settings.fixModelStandard ?? ""} onChange={(fixModelStandard) => change({ fixModelStandard })}
                effort={settings.fixEffortStandard ?? ""} onEffort={(fixEffortStandard) => change({ fixEffortStandard })}
                help="Ordinary bug fixes in a few files that need some investigation." />
              <ModelSetting title="Model for complex fixes" value={settings.fixModelComplex ?? ""} onChange={(fixModelComplex) => change({ fixModelComplex })}
                effort={settings.fixEffortComplex ?? ""} onEffort={(fixEffortComplex) => change({ fixEffortComplex })}
                help="Used when a standard fix came back after an earlier fix, or is tried again after failing. Complex items are left for you to fix." />
              <SwitchRow title="Try a failed fix again with the next level's model" on={settings.fixEscalate ?? true} onChange={(fixEscalate) => change({ fixEscalate })}
                help="If a fix cannot get the tests to pass, it gets one more go on the next level up (quick → standard → complex), building on what the first attempt did." />
              <span className="help">Higher effort means Claude thinks for longer before answering: better on hard problems, but slower. Stronger models and higher effort use a Claude subscription's limits faster; if a limit is reached, Warden pauses rather than failing items.</span>
              <SwitchRow title="Triage dry run" on={settings.triageDryRun} onChange={(triageDryRun) => change({ triageDryRun })}
                help="Nothing goes to Azure DevOps: items are not tagged or commented on, and fixes are tested but never committed, pushed or opened as pull requests. Separate from the pull request dry run." />
              <span className="help">Needs your token to have Work Items (Read &amp; Write). Pause everything from the button in the sidebar's monitoring box.</span>
            </div>
          </section>
          <section className="setting-group">
            <div><h2>Triage prompt</h2><span className="help">How Claude rates each work item.</span></div>
            <PromptSetting value={settings.triagePrompt} onSave={(triagePrompt) => change({ triagePrompt })} loadDefault={api.defaultTriagePrompt}
              help={<>Warden always adds the work item's details, the mapped repository, and the answer format, so you only need to say how to rate risk and suitability.
                An item is only marked AI-fixable when Claude rates it low risk, suitable and high confidence.</>} />
          </section>
        </>
      )}

      {tab === "triage" && triageOn && (
        <section className="setting-group">
          <div><h2>Fix prompt</h2><span className="help">How Claude goes about fixing an item, automatically or with “Fix with AI”.</span></div>
          <PromptSetting value={settings.fixPrompt ?? ""} onSave={(fixPrompt) => change({ fixPrompt })} loadDefault={api.defaultFixPrompt}
            help={<>Warden always adds the work item with its comments and parent, the triage notes, your notes, how to build and run the tests, and the report format.
              It also keeps Claude inside the item's checkout, with no git and no weakening tests. The fix is committed and a pull request opened only if the tests pass.</>} />
        </section>
      )}

      {tab === "triage" && triageOn && (
        <section className="setting-group">
          <div><h2>Batch fix prompt</h2><span className="help">How Claude goes about several items picked on the board and fixed together.</span></div>
          <PromptSetting value={settings.batchPrompt ?? ""} onSave={(batchPrompt) => change({ batchPrompt })} loadDefault={api.defaultBatchPrompt}
            help={<>Used instead of the fix prompt when you pick items with their tick boxes and choose “Batch fix”. Items in the same repository and branch
              are fixed in one change, with one pull request linked to all of them; Warden adds every item, the same rules and the test run as for one.</>} />
        </section>
      )}

      {tab === "projects" && (
        <section className="setting-group">
          <div><h2>Projects</h2><span className="help">Each project is set up on its own: whether its pull requests are reviewed and how, and whether its board is triaged and where its code lives.</span></div>
          <div className="bare"><ProjectsSettings settings={settings} change={change} triageOn={triageOn} /></div>
        </section>
      )}
    </div>
  );
}

export function Setup({ settings, change, onFinish }: { settings: Settings; change: Change; onFinish: () => void }) {
  const [step, setStep] = useState(0);
  const [connected, setConnected] = useState(false);
  const [projectChoices, setProjectChoices] = useState<string[]>([]);
  const [claude, recheck] = useClaudeStatus("setup");
  const labels = ["Connect", "Scope", "Claude Code"];
  const ready = step === 0 ? connected : step === 2 ? !!claude?.found && claude.supported && claude.signedIn && claude.gitFound : true;

  return (
    <div className="setup">
      <div className="drag" data-tauri-drag-region />
      <div className="setup-scroll">
        <div className="setup-inner">
          <div className="stack" style={{ gap: 14, alignItems: "stretch" }}>
            <div className="logo mono">W</div>
            <h1>Set up Warden</h1>
            <div className="tone-muted" style={{ lineHeight: 1.55 }}>
              Warden watches Azure DevOps for new pull requests, has Claude Code review each one, then posts the findings for you.
            </div>
            <div className="progress mono">
              {labels.map((label, k) => (
                <div key={label} className={`${k <= step ? "reached" : ""} ${k === step ? "now" : ""}`}><i />{k + 1}  {label}</div>
              ))}
            </div>
          </div>

          {step === 0 && <Connect settings={settings} change={change} onConnected={(ok, projects) => { setConnected(ok); setProjectChoices(projects); }} />}
          {step === 1 && <Scope settings={settings} change={change} projectChoices={projectChoices} showExisting />}
          {step === 2 && (
            <div className="fields" style={{ gap: 14 }}>
              <ClaudeCheck status={claude} onRecheck={recheck} />
              <SwitchRow title="Start in dry-run mode" on={settings.dryRun} onChange={(dryRun) => change({ dryRun })}
                help="Reviews run but nothing is posted until you turn this off in Settings." />
              <ApproveSetting settings={settings} change={change} />
            </div>
          )}

          <div className="setup-foot">
            <button className="btn ghost" style={{ visibility: step > 0 ? "visible" : "hidden" }} onClick={() => setStep(step - 1)}>Back</button>
            <div className="inline">
              {!ready && <span className="help">{step === 0 ? "Test the connection to continue" : "Claude Code must be installed, up to date and signed in"}</span>}
              <button className="btn primary" disabled={!ready} onClick={() => (step < 2 ? setStep(step + 1) : onFinish())}>
                {step < 2 ? "Continue" : "Start monitoring"}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
