import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { HistoryRecord, Live, Settings, Severity } from "./types";
import { Avatar, CheckButton } from "./ui";
import { SEVERITIES, STEPS, ago, clock, countsLabel, decisionLabel, duration, location, outcomeLabel } from "./util";

/** Renders the small subset of Markdown that posted comments use. */
function Markdown({ text }: { text: string }) {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return (
    <>
      {parts.map((part, i) =>
        part.startsWith("**") ? <b key={i} style={{ color: "var(--text)" }}>{part.slice(2, -2)}</b>
          : part.startsWith("`") ? <code key={i} className="mono" style={{ fontSize: 11, background: "var(--raise)", padding: "1px 4px", borderRadius: 3 }}>{part.slice(1, -1)}</code>
          : part,
      )}
    </>
  );
}

const SEVERITY_NOTE: Record<Severity, string> = {
  critical: "Must fix, blocks approval",
  major: "Blocks approval",
  minor: "Suggestion, doesn't block",
};

const tone = (r: HistoryRecord) => (r.status === "approved" ? "ok" : r.status === "rejected" ? "critical" : "major");

function PrId({ id }: { id: number }) {
  return <span className="id mono"> #{id}</span>;
}

export function Activity({ live, history, settings, now, onOpen, onHistory }: {
  live: Live; history: HistoryRecord[]; settings: Settings; now: number; onOpen: (recordId: string) => void; onHistory: () => void;
}) {
  const logRef = useRef<HTMLDivElement>(null);
  const current = live.current;
  const lineCount = current?.lines.length ?? 0;
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lineCount]);

  const result = current?.done ? history.find((r) => r.recordId === current.recordId) : undefined;
  const elapsed = current ? Math.max(0, Math.round(((result ? new Date(result.finishedAt).getTime() : now) - new Date(current.startedAt).getTime()) / 1000)) : 0;

  return (
    <>
      {live.error && (
        <div className="banner tint-critical" role="alert">
          <b className="tone-critical">Last check failed</b>
          <span className="grow selectable">{live.error}</span>
          <CheckButton checking={live.checking} label="Try again" />
        </div>
      )}
      {settings.dryRun && (
        <div className="banner tint-major">
          <b className="tone-major">Dry run</b>
          <span className="grow">Reviews run, but nothing is posted, voted on or completed. Turn this off in Settings.</span>
        </div>
      )}
      <div className="activity">
        <div className="card live">
          {current ? (
            <>
              <div className="live-head">
                <div className="live-label mono">
                  <span className={!current.done ? "" : current.failed.length ? "tone-critical" : "tone-muted"} style={current.done ? undefined : { color: "var(--accent-fg)" }}>
                    {!current.done ? "Reviewing now" : result?.status === "failed" ? "Review failed" : current.failed.length ? "Review complete, with problems" : "Review complete"}
                  </span>
                  <span className="tone-faint">{clock(elapsed)}</span>
                </div>
                <div className="inline" style={{ gap: 12 }}>
                  <Avatar name={current.pr.author} size={32} />
                  <div className="stack" style={{ gap: 3 }}>
                    <div className="live-title selectable">{current.pr.title}<PrId id={current.pr.id} /></div>
                    <div className="meta mono">{current.pr.author} · {current.pr.project}/{current.pr.repo} · {current.pr.source} → {current.pr.target}</div>
                  </div>
                </div>
                <div className="steps">
                  {STEPS.map((label, k) => {
                    const state = current.failed.includes(k) ? "failed"
                      : current.skipped.includes(k) ? "skipped"
                      : k < current.step ? "done"
                      : k === current.step && !current.done ? "now" : "";
                    const note = state === "failed" ? " (failed)" : state === "skipped" ? " (skipped)" : "";
                    return (
                      <div key={label} className={`step ${state}`} title={`${label}${note}`}>
                        <div className="bar" /><span className="ellipsis">{label}{state === "failed" ? " ✕" : ""}</span>
                      </div>
                    );
                  })}
                </div>
              </div>
              {result && (
                <div className={`result tint-${tone(result)}`}>
                  <b className={`tone-${tone(result)}`}>{decisionLabel(result)}</b>
                  <span className="tone-muted" style={{ flex: 1 }}>
                    {result.status === "failed" ? "See the log below" : countsLabel(result)} · {outcomeLabel(result)}
                  </span>
                  <button className="link" onClick={() => onOpen(result.recordId)}>View review →</button>
                </div>
              )}
              <div className="log mono" ref={logRef}>
                {current.lines.map((l, i) => (
                  <div className="log-line" key={i}><span className="time">{l.time}</span><span className={`text k-${l.kind}`}>{l.text}</span></div>
                ))}
                {!current.done && <div className="log-line"><span className="time" style={{ visibility: "hidden" }}>00:00:00</span><span className="cursor" /></div>}
              </div>
            </>
          ) : (
            <div className="empty">
              <b>{live.status === "paused" ? "Monitoring is paused" : "Watching for new pull requests"}</b>
              <span>
                {live.status === "paused"
                  ? "No pull requests are being checked or reviewed."
                  : live.checking ? "Checking Azure DevOps for new pull requests…"
                  : !live.lastCheck ? "Waiting for the first check."
                  : `Last checked ${ago(live.lastCheck, now)} and found nothing new. New pull requests are reviewed as they appear.`}
              </span>
              {!settings.reviewExisting && live.status !== "paused" && (
                <span className="help">Pull requests that were already open when monitoring started are left alone.</span>
              )}
              {live.status !== "paused" && <CheckButton checking={live.checking} />}
            </div>
          )}
        </div>

        <div className="rail">
          <div className="rail-group">
            <div className="section-label mono">Up next</div>
            {live.queue.length === 0 && <span className="help">Nothing waiting.</span>}
            {live.queue.map((q) => (
              <div className="card queue-item" key={q.id}>
                <Avatar name={q.author} />
                <div className="stack" style={{ gap: 3 }}>
                  <span style={{ lineHeight: 1.35 }}>{q.title}</span>
                  <span className="count mono">#{q.id} · {q.repo}</span>
                </div>
              </div>
            ))}
          </div>
          {live.claimed.length > 0 && (
            <div className="rail-group">
              <div className="section-label mono">Being reviewed elsewhere</div>
              {live.claimed.map((c) => (
                <div className="card queue-item" key={c.pr.id}>
                  <Avatar name={c.by} />
                  <div className="stack" style={{ gap: 3 }}>
                    <span style={{ lineHeight: 1.35 }}>{c.pr.title}</span>
                    <span className="count mono">#{c.pr.id} · by {c.by}'s Warden</span>
                  </div>
                </div>
              ))}
            </div>
          )}
          <div className="rail-group">
            <div className="section-label mono row-between"><span>Recently reviewed</span><button className="link" onClick={onHistory}>All</button></div>
            {history.length === 0 && <span className="help">No reviews yet.</span>}
            {history.slice(0, 4).map((r) => (
              <button className="recent" key={r.recordId} onClick={() => onOpen(r.recordId)}>
                <div className="row-between" style={{ fontSize: 11 }}>
                  <b className={`tone-${tone(r)}`}>{decisionLabel(r)}</b><span className="count mono">{ago(r.finishedAt, now)}</span>
                </div>
                <span style={{ lineHeight: 1.35 }}>{r.pr.title}</span>
                <span className="count mono">{r.pr.author}{r.review ? ` · ${countsLabel(r, true)}` : ""}</span>
              </button>
            ))}
          </div>
        </div>
      </div>
    </>
  );
}

export function History({ rows, now, onOpen, emptyHint }: {
  rows: { latest: HistoryRecord; count: number }[]; now: number; onOpen: (recordId: string) => void; emptyHint: string;
}) {
  return (
    <div>
      <div className="history-head section-label mono">
        <span>Decision</span><span>Pull request</span><span>Raised by</span><span>Issues</span><span style={{ textAlign: "right" }}>Reviewed</span>
      </div>
      {rows.map(({ latest: r, count }) => (
        <button className="history-row" key={r.pr.id} onClick={() => onOpen(r.recordId)}>
          <div className="stack">
            <span className={`pill tone-${tone(r)} tint-${tone(r)}`}>{decisionLabel(r)}</span>
            <span className="count">{outcomeLabel(r)}</span>
          </div>
          <div className="stack">
            <span className="ellipsis" style={{ fontWeight: 500, maxWidth: "100%" }}>{r.pr.title}</span>
            <span className="count mono ellipsis" style={{ maxWidth: "100%" }}>
              #{r.pr.id} · {r.pr.project}/{r.pr.repo}{r.stats ? ` · +${r.stats.additions} −${r.stats.deletions}` : ""}{count > 1 ? ` · reviewed ${count} times` : ""}
            </span>
          </div>
          <div className="inline" style={{ gap: 8, minWidth: 0 }}><Avatar name={r.pr.author} /><span className="ellipsis">{r.pr.author}</span></div>
          <div className="sev-counts mono" aria-label={r.review ? countsLabel(r) : "no review"}>
            {r.review ? SEVERITIES.map((s) => (
              <span key={s.key} className={r.counts[s.key] ? "" : "tone-faint"} title={s.label}>
                <i className="dot" style={{ width: 6, height: 6, background: r.counts[s.key] ? `var(--${s.key})` : "var(--line)" }} />{r.counts[s.key]}
              </span>
            )) : <span className="tone-faint">–</span>}
          </div>
          <span className="meta mono" style={{ textAlign: "right" }}>{ago(r.finishedAt, now)}</span>
        </button>
      ))}
      {rows.length === 0 && <div className="empty">{emptyHint}</div>}
    </div>
  );
}

/** Shown when a review finished but no vote was cast, with a way to cast it now. */
function VoteNow({ record: r, onSettings }: { record: HistoryRecord; onSettings: () => void }) {
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  const approve = r.status === "approved";
  const cast = () => {
    setState({ busy: true, error: null });
    api.applyVote(r.recordId).then(() => setState({ busy: false, error: null }), (e) => setState({ busy: false, error: String(e) }));
  };
  return (
    <div className="warning tint-major">
      <span>
        <b className="tone-major">{approve ? "Not approved on Azure DevOps yet." : "No vote was cast."}</b>{" "}
        Warden only commented, because "Vote, and auto-complete clean PRs" was off when this review ran.
      </span>
      <div className="inline" style={{ flexWrap: "wrap" }}>
        <button className="btn primary" disabled={state.busy} onClick={cast}>
          {state.busy ? "Working…" : approve ? "Approve and set auto-complete" : "Vote Waiting for author"}
        </button>
        <button className="link" onClick={onSettings}>Turn it on for future reviews</button>
      </div>
      {state.error && <span className="tone-critical selectable" role="alert">{state.error}</span>}
    </div>
  );
}

export function Detail({ record: r, reviews, now, onOpen, onRetry, onSettings, pendingFix, onFix, onShowFixes }: {
  record: HistoryRecord; reviews: HistoryRecord[]; now: number; onOpen: (recordId: string) => void; onRetry: () => void; onSettings: () => void;
  /** A fix for this pull request is already being prepared or awaiting approval. */
  pendingFix: boolean; onFix: (issues: number[]) => Promise<void>; onShowFixes: () => void;
}) {
  const t = tone(r);
  const first = r.pr.author.split(" ")[0];
  const blocking = r.counts.critical + r.counts.major;
  const criteria = r.review?.criteria ?? [];
  const unmet = criteria.filter((c) => c.status === "not_met").length;
  const criterionTone = { met: "ok", not_met: "critical", unclear: "major" } as const;
  const criterionLabel = { met: "Met", not_met: "Not met", unclear: "Unclear" } as const;
  const criterionGlyph = { met: "✓", not_met: "✕", unclear: "?" } as const;
  const met = criteria.filter((c) => c.status === "met").length;
  const unclear = criteria.filter((c) => c.status === "unclear").length;
  const acTone = unmet ? "critical" : unclear ? "major" : "ok";
  // Sections start open; the choice resets when another review is shown.
  const [collapsed, setCollapsed] = useState<Partial<Record<Severity, boolean>>>({});
  useEffect(() => setCollapsed({}), [r.recordId]);
  const jump = (target: "ac" | Severity) => {
    if (target !== "ac") setCollapsed((c) => ({ ...c, [target]: false }));
    setTimeout(() => document.getElementById(`sec-${target}`)?.scrollIntoView({ behavior: "smooth", block: "start" }), 30);
  };
  const tiles: { label: string; value: string; sub: string; tone: string | null; target: "ac" | Severity }[] = [
    {
      label: "Acceptance", target: "ac",
      value: criteria.length ? `${met}/${criteria.length}` : "—",
      sub: !criteria.length ? "none linked" : unmet ? `${unmet} not met` : unclear ? `${unclear} unclear` : "all met",
      tone: criteria.length ? acTone : null,
    },
    ...SEVERITIES.map((sev) => ({
      label: sev.label, target: sev.key, value: String(r.counts[sev.key]), sub: SEVERITY_NOTE[sev.key].split(",")[0].toLowerCase(),
      tone: r.counts[sev.key] ? sev.key : null,
    })),
  ];
  const headline =
    r.status === "failed" ? "Review failed"
      : r.status === "approved" ? (r.merged ? "Approved and merged" : r.autoComplete ? "Approved and set to auto-complete" : r.vote ? "Approved" : "Approval recommended")
      : "Rejected: changes required";
  const subline =
    r.status === "failed" ? "Nothing was posted to the pull request."
      : r.status === "approved" ? `No critical or major issues · ${r.counts.minor} minor left as suggestions`
      : unmet ? `${unmet} acceptance criteri${unmet > 1 ? "a" : "on"} not met${blocking ? ` · ${blocking} critical or major issue${blocking > 1 ? "s" : ""}` : ""}`
      : blocking ? `${blocking} critical or major issue${blocking > 1 ? "s" : ""} must be fixed before merge`
      : "Claude rejected this change; see the summary";
  const voteLabel = { approved: "Approved", approvedWithSuggestions: "Approved with suggestions", waitingForAuthor: "Waiting for author" };
  const timeline: { label: string; value: string; tone?: string }[] = [
    { label: "Review started", value: new Date(r.startedAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) },
    ...(r.pr.commit ? [{ label: "Commit reviewed", value: r.pr.commit.slice(0, 7) }] : []),
    { label: "Claude Code review", value: duration(r.durationSecs) },
    ...(r.status !== "failed" ? [
      { label: "Comment", value: r.dryRun ? "dry run" : r.posted ? `posted${first ? ` · @${first}` : ""}` : "not posted", tone: r.posted || r.dryRun ? undefined : "critical" },
      { label: "Vote", value: r.vote ? voteLabel[r.vote] : "none", tone: r.vote ? (r.vote === "waitingForAuthor" ? "critical" : "ok") : undefined },
    ] : []),
    ...(r.autoComplete ? [{ label: "Auto-complete", value: r.merged ? `merged into ${r.pr.target}` : "set", tone: r.merged ? "ok" : undefined }] : []),
  ];
  // Each issue keeps its position in the review, which is how a fix refers to it.
  const indexed = (r.review?.comments ?? []).map((issue, index) => ({ issue, index }));
  const groups = SEVERITIES.map((s) => ({ ...s, issues: indexed.filter(({ issue }) => issue.severity === s.key) })).filter((g) => g.issues.length);
  // Blocking issues start ticked; they are the ones that failed the pull request.
  const blockingIndexes = indexed.filter(({ issue }) => issue.severity !== "minor").map(({ index }) => index);
  const [picked, setPicked] = useState<number[]>(blockingIndexes);
  const [fixState, setFixState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  useEffect(() => {
    setPicked(blockingIndexes);
    setFixState({ busy: false, error: null });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [r.recordId]);
  const togglePicked = (index: number) => setPicked((p) => (p.includes(index) ? p.filter((i) => i !== index) : [...p, index]));
  const startFix = () => {
    setFixState({ busy: true, error: null });
    onFix([...picked].sort((x, y) => x - y)).then(() => setFixState({ busy: false, error: null }), (e) => setFixState({ busy: false, error: String(e) }));
  };

  return (
    <div className="detail">
      <div className="detail-main">
        <div className="stack" style={{ gap: 10 }}>
          <div className="detail-title selectable">{r.pr.title}<span className="id mono" style={{ fontSize: 15 }}> #{r.pr.id}</span></div>
          <div className="detail-meta">
            <span className="inline" style={{ gap: 6 }}><Avatar name={r.pr.author} size={20} />{r.pr.author}</span>
            <span className="mono">{r.pr.project}/{r.pr.repo}</span>
            <span className="mono">{r.pr.source} → {r.pr.target}</span>
            {r.stats && <span className="mono">{r.stats.files} files · <span className="tone-ok">+{r.stats.additions}</span> <span className="tone-critical">−{r.stats.deletions}</span></span>}
          </div>
        </div>
        <div className="card decision">
          <div className={`decision-head tint-${t}`}>
            <span className="glyph-tile" style={{ width: 34, height: 34, borderRadius: 10, fontSize: 16, background: `var(--${t})` }} aria-hidden>
              {r.status === "approved" ? "✓" : r.status === "rejected" ? "✕" : "!"}
            </span>
            <div className="stack" style={{ gap: 3 }}>
              <b className={`tone-${t}`} style={{ fontSize: 15 }}>{headline}{r.dryRun && r.status !== "failed" ? " (dry run)" : ""}</b>
              <span className="tone-muted" style={{ lineHeight: 1.45 }}>{subline}</span>
            </div>
          </div>
          {r.review && (
            <div className="tiles">
              {tiles.map((tile) => (
                <button className="tile" key={tile.label} onClick={() => jump(tile.target)} title={`Go to ${tile.label}`}>
                  <span className="section-label mono">{tile.label}</span>
                  <span className="tile-value">
                    <b style={{ color: tile.tone ? `var(--${tile.tone})` : "var(--faint)" }}>{tile.value}</b>
                    <span>{tile.sub}</span>
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>
        {r.review && !r.vote && !r.dryRun && r.status !== "failed" && <VoteNow record={r} onSettings={onSettings} />}
        {r.error && (
          <div className="warning tint-critical" role="alert">
            <b className="tone-critical">{r.status === "failed" ? "What went wrong" : "Not everything went through"}</b>
            <span className="selectable" style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{r.error}</span>
          </div>
        )}
        {r.review && (
          <div className="stack" style={{ gap: 8 }}>
            <div className="section-label mono">Summary</div>
            <div className="summary selectable">{r.review.summary}</div>
          </div>
        )}
        {r.review && (
          <section id="sec-ac" className="card section-card" style={unmet ? { borderColor: "color-mix(in oklab, var(--critical) 45%, var(--line))" } : undefined}>
            <div className="section-head" style={{ flexDirection: "column", alignItems: "stretch", gap: 12 }}>
              <div className="inline" style={{ flexWrap: "wrap" }}>
                <span className="glyph-tile" style={{ width: 22, height: 22, borderRadius: 7, fontSize: 12, background: "var(--text)", color: "var(--panel)" }} aria-hidden>!</span>
                <span className="section-title" style={{ fontSize: 15 }}>Acceptance criteria</span>
                <span className={`badge-soft ${criteria.length ? `tone-${acTone} tint-${acTone}` : ""}`}>
                  {criteria.length ? `${met} of ${criteria.length} met` : "Not checked"}
                </span>
                <span className="spacer" />
                <span className="section-label mono">Checked first</span>
              </div>
              {criteria.length > 0 && (
                <div className="ac-bar" aria-hidden>
                  {criteria.map((c, i) => <i key={i} style={{ background: `var(--${criterionTone[c.status]})` }} />)}
                </div>
              )}
              {(r.workItems ?? []).length > 0 && (
                <div className="help selectable">Checked against <span style={{ color: "var(--text)" }}>{r.workItems.map((w) => `${w.kind} ${w.id}: ${w.title}`).join(" · ")}</span></div>
              )}
            </div>
            {criteria.map((c, i) => (
              <div className="ac-row" key={i} style={c.status === "not_met" ? { background: "color-mix(in oklab, var(--critical) 6%, transparent)" } : undefined}>
                <span className="glyph-tile" style={{ background: `var(--${criterionTone[c.status]})` }} aria-hidden>{criterionGlyph[c.status]}</span>
                <div className="stack" style={{ gap: 4 }}>
                  <span className="selectable" style={{ fontWeight: 500, lineHeight: 1.45 }}>{c.criterion}</span>
                  {c.note && <span className="selectable tone-muted" style={{ fontSize: 12.5, lineHeight: 1.55 }}>{c.note}</span>}
                </div>
                <div className="stack" style={{ alignItems: "flex-end", gap: 6 }}>
                  <span className={`badge-soft tone-${criterionTone[c.status]} tint-${criterionTone[c.status]}`}>{criterionLabel[c.status]}</span>
                  {c.work_item && <span className="count mono">#{c.work_item}</span>}
                </div>
              </div>
            ))}
            {criteria.length === 0 && (
              <div className="ac-row tone-muted" style={{ display: "block", fontSize: 12.5 }}>
                {(r.workItems ?? []).length === 0
                  ? "No work items are linked to this pull request, so it was reviewed on code quality only."
                  : "The linked work items have no acceptance criteria, so it was reviewed on code quality only."}
              </div>
            )}
          </section>
        )}
        {r.review && groups.length === 0 && <span className="tone-ok">No issues found.</span>}
        {groups.length > 0 && (
          <div className="fix-bar card">
            <div className="stack" style={{ gap: 3, flex: 1 }}>
              <b>Fix with Claude</b>
              <span className="help">
                {pendingFix ? "A fix for this pull request is already in progress or waiting for your review."
                  : "Tick the issues to fix. Claude prepares the changes for you to read; nothing is pushed until you approve them."}
              </span>
              {fixState.error && <span className="tone-critical selectable" role="alert">{fixState.error}</span>}
            </div>
            {pendingFix ? <button className="btn" onClick={onShowFixes}>View fixes</button> : (
              <button className="btn primary" disabled={fixState.busy || picked.length === 0} onClick={startFix}>
                {fixState.busy ? "Starting…" : picked.length ? `Fix ${picked.length} selected` : "Select issues"}
              </button>
            )}
          </div>
        )}
        {groups.map((g) => {
          const open = !collapsed[g.key];
          return (
            <section id={`sec-${g.key}`} className="card section-card" key={g.key}>
              <button className="section-head" onClick={() => setCollapsed({ ...collapsed, [g.key]: open })} aria-expanded={open}>
                <i style={{ width: 10, height: 10, borderRadius: 3, background: `var(--${g.key})`, flex: "none" }} />
                <span className="section-title">{g.label}</span>
                <span className={`badge-soft mono tone-${g.key} tint-${g.key}`}>{g.issues.length}</span>
                <span className="tone-muted" style={{ fontSize: 12 }}>{SEVERITY_NOTE[g.key]}</span>
                <span className="spacer" />
                <span className="tone-faint" style={{ fontSize: 12 }}>{open ? "Hide ▾" : "Show ▸"}</span>
              </button>
              {open && g.issues.map(({ issue, index }) => (
                <div className="issue-row" key={index}>
                  <div className="row-between" style={{ gap: 16 }}>
                    <label className="inline" style={{ gap: 10, alignItems: "baseline", minWidth: 0 }}>
                      <input type="checkbox" checked={picked.includes(index)} onChange={() => togglePicked(index)} disabled={pendingFix}
                        aria-label={`Include in fix: ${issue.title ?? issue.body.split("\n")[0]}`} />
                      <span style={{ fontWeight: 500 }} className="selectable">{issue.title ?? issue.body.split("\n")[0]}</span>
                    </label>
                    <span className="mono" style={{ fontSize: 11, color: "var(--accent-fg)", flex: "none" }} title={issue.file ?? undefined}>{location(issue.file, issue.line)}</span>
                  </div>
                  {issue.title && <div className="issue-body selectable">{issue.body}</div>}
                  {issue.snippet && <pre className="snippet mono">{issue.snippet}</pre>}
                </div>
              ))}
            </section>
          );
        })}
      </div>

      <div className="aside">
        <div className="inline">
          <button className="btn" onClick={() => api.open(r.pr.url)}>Open in Azure DevOps ↗</button>
          <button className="btn" onClick={onRetry} title="Review this pull request again on the next check">Review again</button>
        </div>
        {r.comment && (
          <div className="stack" style={{ gap: 8, alignItems: "stretch" }}>
            <div className="section-label mono">{r.posted ? "Comment posted to PR" : "Comment (not posted)"}</div>
            <div className="card" style={{ overflow: "hidden" }}>
              <div className="comment-head">
                <span className="mono logo" style={{ width: 20, height: 20, fontSize: 10, borderRadius: 6 }}>W</span>
                <b>Warden</b><span className="tone-faint">via your token</span>
              </div>
              <div className="comment-text"><Markdown text={r.comment.replace(/@<[^>]+>/g, `@${r.pr.author}`)} /></div>
            </div>
          </div>
        )}
        <div className="stack" style={{ gap: 8, alignItems: "stretch" }}>
          <div className="section-label mono">Timeline</div>
          <div>
            {timeline.map((e) => (
              <div className="timeline-row" key={e.label}>
                <i className="dot" style={{ width: 6, height: 6, background: e.tone ? `var(--${e.tone})` : "var(--faint)" }} />
                <span className="label">{e.label}</span>
                <span className={`mono ${e.tone ? `tone-${e.tone}` : ""}`} style={{ fontSize: 11 }}>{e.value}</span>
              </div>
            ))}
          </div>
        </div>
        {reviews.length > 1 && (
          <div className="stack" style={{ gap: 8, alignItems: "stretch" }}>
            <div className="section-label mono">Reviews of this pull request</div>
            <div>
              {reviews.map((other, i) => (
                <button key={other.recordId} className="timeline-row" style={{ width: "100%" }} disabled={other.recordId === r.recordId}
                  onClick={() => onOpen(other.recordId)} aria-current={other.recordId === r.recordId}>
                  <i className="dot" style={{ width: 6, height: 6, background: `var(--${tone(other)})` }} />
                  <span className="label" style={other.recordId === r.recordId ? { color: "var(--text)", fontWeight: 500 } : undefined}>
                    {decisionLabel(other)}{i === 0 ? " · latest" : ""}{other.recordId === r.recordId ? " · shown" : ""}
                  </span>
                  <span className="mono tone-faint" style={{ fontSize: 11 }}>{other.pr.commit ? `${other.pr.commit.slice(0, 7)} · ` : ""}{ago(other.finishedAt, now)}</span>
                </button>
              ))}
            </div>
          </div>
        )}
        {r.lines.length > 0 && (
          <details>
            <summary className="section-label mono" style={{ cursor: "pointer" }}>Review log</summary>
            <div className="log mono" style={{ height: 220, border: "1px solid var(--line)", borderRadius: 8, marginTop: 8, padding: 10, fontSize: 11 }}>
              {r.lines.map((l, i) => <div className="log-line" key={i}><span className="time">{l.time}</span><span className={`text k-${l.kind}`}>{l.text}</span></div>)}
            </div>
          </details>
        )}
      </div>
    </div>
  );
}
