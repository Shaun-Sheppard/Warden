import { useEffect, useState } from "react";
import { api, type AvailableUpdate } from "./api";
import type { ClaudeStatus, Settings } from "./types";
import { Segmented, SwitchRow, TagInput } from "./ui";

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
              <div className={status.signedIn ? "tone-ok" : "tone-critical"}>{status.version || "unknown"} · {status.signedIn ? "signed in" : "not signed in"}</div>
            </>
          )}
          {!status.gitFound && <div className="tone-critical">git was not found</div>}
        </>
      )}
      {status && (!status.found || !status.signedIn || !status.gitFound) && (
        <div style={{ marginTop: 8, fontFamily: "inherit" }}>
          <span className="tone-muted">
            {!status.found ? "Install the Claude Code CLI, or set its path in Settings." : !status.signedIn ? "Run `claude auth login` in a terminal." : "Install git."}{" "}
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

/** Editor for the review guidance, starting from the built-in text. */
function PromptSetting({ settings, change }: { settings: Settings; change: Change }) {
  const [builtIn, setBuiltIn] = useState("");
  const [text, setText] = useState<string | null>(null);
  useEffect(() => {
    api.defaultPrompt().then((value) => {
      setBuiltIn(value);
      setText((current) => current ?? (settings.reviewPrompt.trim() ? settings.reviewPrompt : value));
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  if (text === null) return null;
  const custom = settings.reviewPrompt.trim() !== "";
  const dirty = text.trim() !== (custom ? settings.reviewPrompt.trim() : builtIn.trim());
  // Saving text identical to the built-in guidance keeps "default", so future improvements to it still apply.
  const save = () => change({ reviewPrompt: text.trim() === builtIn.trim() ? "" : text.trim() });
  return (
    <div className="fields">
      <label className="field"><span>Instructions for Claude</span>
        <textarea className="input mono prompt" value={text} spellCheck={false} onChange={(e) => setText(e.target.value)} />
      </label>
      <span className="help">
        Warden always adds the pull request's details, its linked work items and acceptance criteria, the repo's REVIEW.md, and the answer format, so you only need to say what to look for.
        Keep the Critical / Major / Minor wording: approval depends on those levels.
      </span>
      <div className="inline">
        <button className="btn primary" disabled={!dirty || !text.trim()} onClick={save}>Save prompt</button>
        <button className="btn" disabled={!custom && !dirty} onClick={() => { setText(builtIn); change({ reviewPrompt: "" }); }}>Reset to default</button>
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
  percent: number | null;
  error: string;
  /** A review is running; installing waits until it finishes. */
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
            <span><span className="spinner" aria-hidden /> Downloading{offer.percent === null ? "…" : ` ${offer.percent}%`} · Warden will restart when it is ready.</span>
          ) : (
            <div className="inline" style={{ flexWrap: "wrap" }}>
              <button className="btn primary" onClick={offer.install} disabled={offer.reviewing}>{offer.phase === "error" ? "Try again" : "Update and restart"}</button>
              {offer.reviewing && <span className="help">Available once the current review finishes</span>}
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

export function SettingsView({ settings, change, onRunSetup, offer, onCheck }: {
  settings: Settings; change: Change; onRunSetup: () => void; offer: UpdateOffer; onCheck: () => Promise<AvailableUpdate | null>;
}) {
  const [projectChoices, setProjectChoices] = useState<string[]>([]);
  const [claude, recheck] = useClaudeStatus(settings.cliPath);
  return (
    <div className="settings">
      <section className="setting-group">
        <div><h2>Azure DevOps</h2><span className="help">Where Warden looks for pull requests.</span></div>
        <Connect settings={settings} change={change} onConnected={(_, projects) => setProjectChoices(projects)} />
      </section>

      <section className="setting-group">
        <div><h2>Scope</h2><span className="help">Leave either list empty to include everything.</span></div>
        <Scope settings={settings} change={change} projectChoices={projectChoices} />
      </section>

      <section className="setting-group">
        <div><h2>Review behaviour</h2><span className="help">What happens after Claude decides.</span></div>
        <div className="fields">
          <div className="switch-row"><span className="label">Check for new PRs every</span>
            <Segmented label="Check interval" value={settings.pollSeconds} onChange={(pollSeconds) => change({ pollSeconds })}
              options={[{ value: 30, label: "30s" }, { value: 60, label: "1m" }, { value: 300, label: "5m" }, { value: 900, label: "15m" }]} />
          </div>
          <SwitchRow title="Dry run" on={settings.dryRun} onChange={(dryRun) => change({ dryRun })}
            help="Review pull requests but never post, vote or complete. Use it to see what Warden would do." />
          <SwitchRow title="Notifications" on={settings.notifications} onChange={(notifications) => change({ notifications })}
            help="Show a system notification when a new pull request starts being reviewed, when the review finishes, and when an update is available." />
          <SwitchRow title="Tag the author in the comment" on={settings.mentionAuthor} onChange={(mentionAuthor) => change({ mentionAuthor })} />
          <ApproveSetting settings={settings} change={change} />
          <div className={`switch-row ${settings.approveAndComplete ? "" : "dimmed"}`}><span className="label">Merge type</span>
            <Segmented label="Merge type" value={settings.mergeStrategy} onChange={(mergeStrategy) => change({ mergeStrategy })}
              options={[{ value: "squash", label: "Squash" }, { value: "noFastForward", label: "Merge" }, { value: "rebase", label: "Rebase" }]} />
          </div>
          <SwitchRow className={settings.approveAndComplete ? "" : "dimmed"} title="Delete source branch after merge" on={settings.deleteSourceBranch} onChange={(deleteSourceBranch) => change({ deleteSourceBranch })} />
        </div>
      </section>

      <section className="setting-group">
        <div><h2>Review prompt</h2><span className="help">What Claude is asked to check in each pull request.</span></div>
        <PromptSetting settings={settings} change={change} />
      </section>

      <section className="setting-group">
        <div><h2>Claude Code</h2><span className="help">The CLI that runs each review.</span></div>
        <div className="fields">
          <label className="field"><span>CLI path</span>
            <input className="input mono" defaultValue={settings.cliPath} placeholder="Found automatically" spellCheck={false}
              onBlur={(e) => e.target.value.trim() !== settings.cliPath && change({ cliPath: e.target.value.trim() })} />
          </label>
          <ClaudeCheck status={claude} onRecheck={recheck} />
        </div>
      </section>

      <section className="setting-group">
        <div><h2>About</h2><span className="help">Version and updates.</span></div>
        <About offer={offer} onCheck={onCheck} />
      </section>

      <section className="setting-group">
        <div><h2>Appearance</h2></div>
        <div className="fields" style={{ alignItems: "flex-start" }}>
          <Segmented label="Theme" value={settings.theme} onChange={(theme) => change({ theme })}
            options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} />
          <button className="link" style={{ fontSize: 12 }} onClick={onRunSetup}>Run first-time setup again</button>
        </div>
      </section>
    </div>
  );
}

export function Setup({ settings, change, onFinish }: { settings: Settings; change: Change; onFinish: () => void }) {
  const [step, setStep] = useState(0);
  const [connected, setConnected] = useState(false);
  const [projectChoices, setProjectChoices] = useState<string[]>([]);
  const [claude, recheck] = useClaudeStatus("setup");
  const labels = ["Connect", "Scope", "Claude Code"];
  const ready = step === 0 ? connected : step === 2 ? !!claude?.found && claude.signedIn && claude.gitFound : true;

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
              {!ready && <span className="help">{step === 0 ? "Test the connection to continue" : "Claude Code must be installed and signed in"}</span>}
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
