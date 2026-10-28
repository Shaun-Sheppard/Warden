import { useEffect, useRef, useState } from "react";
import { ApproveAnyway } from "./approve";
import { api } from "./api";
import { FixCard } from "./fixes";
import type { Fix, HistoryRecord, Live, Settings, Severity } from "./types";
import { Avatar, CheckButton, Notice, PageHeader } from "./ui";
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

/** Problems with checking, and dry-run mode, above the PR reviews list. */
export function ReviewNotices({ live, settings }: { live: Live; settings: Settings }) {
  return (
    <>
      {live.error && (
        <Notice tone="critical" title={/signed in|too old|not found on PATH/.test(live.error) ? "Reviewing is paused" : "Last check failed"}
          actions={<CheckButton checking={live.checking} label="Try again" />}>
          {live.error}
        </Notice>
      )}
      {settings.dryRun && (
        <Notice tone="major" title="Dry run">Reviews run, but nothing is posted, voted on or completed. Turn this off in Settings.</Notice>
      )}
    </>
  );
}

/** The review running now (or just finished), with its steps and log, and what waits after it. */
export function LivePanel({ live, history, now, onOpen }: {
  live: Live; history: HistoryRecord[]; now: number; onOpen: (recordId: string) => void;
}) {
  const logRef = useRef<HTMLDivElement>(null);
  const current = live.current;
  const lineCount = current?.lines.length ?? 0;
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lineCount]);
  const waiting = live.queue.length + live.claimed.length;
  if (!current && !waiting) return null;

  const result = current?.done ? history.find((r) => r.recordId === current.recordId) : undefined;
  const elapsed = current ? Math.max(0, Math.round(((result ? new Date(result.finishedAt).getTime() : now) - new Date(current.startedAt).getTime()) / 1000)) : 0;
  const queue = (
    waiting > 0 && (
      <div className="queue-strip">
        {live.queue.length > 0 && <span className="eyebrow">Up next</span>}
        {live.queue.map((q) => <span className="chip" key={q.id} title={q.title}><Avatar name={q.author} size={16} /><span className="ellipsis" style={{ maxWidth: 260 }}>{q.title}</span><span className="mono tone-faint">#{q.id}</span></span>)}
        {live.claimed.length > 0 && <span className="eyebrow">Elsewhere</span>}
        {live.claimed.map((c) => <span className="chip" key={c.pr.id} title={`Being reviewed by ${c.by}'s Warden`}><Avatar name={c.by} size={16} /><span className="ellipsis" style={{ maxWidth: 220 }}>{c.pr.title}</span><span className="mono tone-faint">#{c.pr.id}</span></span>)}
      </div>
    )
  );
  if (!current) return <div className="card live">{queue}</div>;

  return (
    <div className="card live">
      <div className="live-head">
        <div className="live-label">
          <span className={`live-chip ${!current.done ? "" : result?.status === "failed" || current.failed.length ? "failed" : "done"}`}>
            {!current.done && <span className="spin" aria-hidden />}
            {!current.done ? "Reviewing now" : result?.status === "failed" ? "Review failed" : current.failed.length ? "Review complete, with problems" : "Review complete"}
          </span>
          <span className="live-clock mono">{clock(elapsed)}</span>
        </div>
        <div className="inline" style={{ gap: 14, alignItems: "flex-start" }}>
          <Avatar name={current.pr.author} size={40} />
          <div className="stack" style={{ gap: 5 }}>
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
      {queue}
    </div>
  );
}

/** "Today", "Yesterday", or the date. */
function dayLabel(iso: string, now: number): string {
  const day = new Date(iso), today = new Date(now);
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((start(today) - start(day)) / 86400000);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  return day.toLocaleDateString([], { weekday: "long", day: "numeric", month: "short" });
}

export function History({ rows, now, onOpen, emptyHint, fixes }: {
  rows: { latest: HistoryRecord; count: number }[]; now: number; onOpen: (recordId: string) => void; emptyHint: string; fixes: Fix[];
}) {
  const days: { name: string; rows: typeof rows }[] = [];
  for (const row of rows) {
    const name = dayLabel(row.latest.finishedAt, now);
    const day = days.find((d) => d.name === name);
    if (day) day.rows.push(row);
    else days.push({ name, rows: [row] });
  }
  return (
    <div className="table-card">
      <div className="table-head history-head"><span>Decision</span><span>Pull request</span><span>Issues</span><span style={{ textAlign: "right" }}>Reviewed</span></div>
      {days.map((day) => (
        <div className="table-group" key={day.name}>
          <div className="table-group-head">{day.name}<span className="num-chip">{day.rows.length}</span></div>
          {day.rows.map(({ latest: r, count }) => {
            const fixReady = fixes.some((f) => f.pr.id === r.pr.id && f.status === "ready");
            return (
              <button className="history-row" key={r.pr.id} onClick={() => onOpen(r.recordId)}>
                <div className="stack" style={{ gap: 5 }}>
                  <span className={`pill tone-${tone(r)} tint-${tone(r)}`}>{decisionLabel(r)}</span>
                  <span className="help" style={{ paddingLeft: 2 }}>{outcomeLabel(r)}</span>
                </div>
                <div style={{ minWidth: 0 }}>
                  <div className="row-title">{r.pr.title}{r.workItems?.[0] && <span className="id"> #{r.workItems[0].id}</span>}</div>
                  <div className="row-meta">
                    <span style={{ color: "var(--ink3)" }}>#{r.pr.id}</span><span>·</span><span>{r.pr.project}/{r.pr.repo}</span>
                    {r.stats && <><span>·</span><span className="tone-ok">+{r.stats.additions}</span><span className="tone-critical">-{r.stats.deletions}</span></>}
                    {count > 1 && <><span>·</span><span>reviewed {count} times</span></>}
                    <span>·</span><span className="who"><Avatar name={r.pr.author} size={18} />{r.pr.author}</span>
                    {fixReady && <span className="tpill tpill-major">Fix to check</span>}
                  </div>
                </div>
                <div className="sev-counts mono" aria-label={r.review ? countsLabel(r) : "no review"}>
                  {r.review ? SEVERITIES.map((s) => (
                    <span key={s.key} className={r.counts[s.key] ? `on-${s.key}` : ""} title={s.label}>
                      <i className="dot" style={{ width: 6, height: 6, background: r.counts[s.key] ? `var(--${s.key === "major" ? "major-dot" : s.key})` : "var(--dash)" }} />{r.counts[s.key]}
                    </span>
                  )) : <span>–</span>}
                </div>
                <span className="when">{ago(r.finishedAt, now)}</span>
              </button>
            );
          })}
        </div>
      ))}
      {rows.length === 0 && <div className="col-empty" style={{ margin: "10px 8px 4px" }}>{emptyHint}</div>}
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

export function Detail({ record: r, reviews, now, onOpen, onBack, onRetry, onSettings, fixes, onFix }: {
  record: HistoryRecord; reviews: HistoryRecord[]; now: number; onOpen: (recordId: string) => void; onBack: () => void; onRetry: () => void; onSettings: () => void;
  /** Fixes from this pull request's reviews, newest first. */
  fixes: Fix[]; onFix: (issues: number[]) => Promise<void>;
}) {
  /** A fix for this pull request is already being prepared or awaiting approval. */
  const pendingFix = fixes.some((f) => f.status === "ready" || f.status === "generating" || f.status === "pushing");
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
      : r.review?.manipulation ? "The pull request contains text addressed to automated reviewers, so it cannot be approved automatically"
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

  const workItem = r.workItems?.[0];
  return (
    <>
    <PageHeader
      crumbs={<button onClick={onBack}>‹ PR reviews</button>}
      title={<span className="selectable">{r.pr.title}{workItem && <span className="id mono" style={{ fontSize: 15, fontWeight: 500, color: "var(--mono-ink)", letterSpacing: 0 }}> #{workItem.id}</span>}</span>}
      sub={
        <span className="detail-meta" style={{ marginTop: 4 }}>
          <span className="who"><Avatar name={r.pr.author} size={20} />{r.pr.author}</span><span>·</span>
          <span>PR #{r.pr.id}</span><span>·</span>
          <span>{r.pr.project}/{r.pr.repo}</span><span>·</span>
          <span>{r.pr.source} → {r.pr.target}</span>
          {r.stats && <><span>·</span><span>{r.stats.files} files <span className="tone-ok">+{r.stats.additions}</span> <span className="tone-critical">-{r.stats.deletions}</span></span></>}
        </span>
      }>
      <button className="btn" onClick={() => api.open(r.pr.url)}>Open in Azure DevOps ↗</button>
      <button className="btn" onClick={onRetry} title="Review this pull request again on the next check">↻ Review again</button>
    </PageHeader>
    <div className="detail">
      <div className="detail-main">
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
        {r.review?.manipulation && (
          <div className="warning tint-critical" role="alert">
            <b className="tone-critical">Possible attempt to steer the review</b>
            <span>This pull request contains text addressed to automated reviewers. It was not followed, and Warden will not approve this pull request automatically. Read the change yourself before deciding.</span>
            <pre className="snippet mono" style={{ whiteSpace: "pre-wrap" }}>{r.review.manipulation}</pre>
          </div>
        )}
        {r.review && !r.vote && !r.dryRun && r.status !== "failed" && !r.review.manipulation && <VoteNow record={r} onSettings={onSettings} />}
        {r.approvedAnyway && (
          <div className="warning tint-ok">
            <b className="tone-ok">Approved anyway</b>
            <span className="selectable">{r.approvedAnyway}</span>
          </div>
        )}
        {r.status === "rejected" && r.review && !r.dryRun && !r.approvedAnyway && r.vote !== "approved" && (
          <div className="warning tint-major">
            <span><b className="tone-major">Failed review.</b> If the issues are not a problem here, approve it anyway with your reason.</span>
            <ApproveAnyway prId={r.pr.id} project={r.pr.project} inline />
          </div>
        )}
        {r.error && (
          <div className="warning tint-critical" role="alert">
            <b className="tone-critical">{r.status === "failed" ? "What went wrong" : "Not everything went through"}</b>
            <span className="selectable" style={{ whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{r.error}</span>
          </div>
        )}
        {r.review && (
          <div className="card stack" style={{ gap: 10, padding: "18px 20px", alignItems: "stretch" }}>
            <div className="eyebrow">Summary</div>
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
        {r.review && groups.length === 0 && <div className="card" style={{ padding: "16px 20px" }}><span className="tone-ok" style={{ fontWeight: 600 }}>No issues found.</span></div>}
        {groups.length > 0 && (
          <section className="card section-card">
            <div className="fix-bar">
              <div className="stack" style={{ gap: 3, flex: 1 }}>
                <span className="section-title">Issues</span>
                <span className="help">
                  {pendingFix ? "A fix for this pull request is in progress or waiting for you to check it, below."
                    : "Tick the issues to fix. Claude prepares the changes for you to read; nothing is pushed until you approve them."}
                </span>
                {fixState.error && <span className="tone-critical selectable" role="alert">{fixState.error}</span>}
              </div>
              {!pendingFix && (
                <button className="btn primary" disabled={fixState.busy || picked.length === 0} onClick={startFix}>
                  {fixState.busy ? "Starting…" : picked.length ? `Fix ${picked.length} selected` : "Select issues"}
                </button>
              )}
            </div>
            {groups.map((g) => {
              const open = !collapsed[g.key];
              return (
                <div id={`sec-${g.key}`} key={g.key}>
                  <button className="section-head" style={{ background: "var(--raise)", padding: "10px 20px" }} onClick={() => setCollapsed({ ...collapsed, [g.key]: open })} aria-expanded={open}>
                    <i style={{ width: 9, height: 9, borderRadius: 3, background: `var(--${g.key === "major" ? "major-dot" : g.key})`, flex: "none" }} />
                    <span style={{ fontWeight: 700 }}>{g.label}</span>
                    <span className={`badge-soft mono tone-${g.key} tint-${g.key}`}>{g.issues.length}</span>
                    <span className="help">{SEVERITY_NOTE[g.key]}</span>
                    <span className="spacer" />
                    <span className="tone-faint" style={{ fontSize: 12 }}>{open ? "Hide" : "Show"}</span>
                  </button>
                  {open && g.issues.map(({ issue, index }) => (
                    <div className="issue-row" key={index}>
                      <div className="row-between" style={{ gap: 16 }}>
                        <label className="inline" style={{ gap: 10, alignItems: "baseline", minWidth: 0 }}>
                          <input type="checkbox" checked={picked.includes(index)} onChange={() => togglePicked(index)} disabled={pendingFix}
                            aria-label={`Include in fix: ${issue.title ?? issue.body.split("\n")[0]}`} />
                          <span style={{ fontWeight: 600 }} className="selectable">{issue.title ?? issue.body.split("\n")[0]}</span>
                        </label>
                        <span className="mono" style={{ fontSize: 11, color: "var(--accent-fg)", flex: "none" }} title={issue.file ?? undefined}>{location(issue.file, issue.line)}</span>
                      </div>
                      {issue.title && <div className="issue-body selectable" style={{ paddingLeft: 25 }}>{issue.body}</div>}
                      {issue.snippet && <pre className="snippet mono" style={{ marginLeft: 25 }}>{issue.snippet}</pre>}
                    </div>
                  ))}
                </div>
              );
            })}
          </section>
        )}
        {fixes.length > 0 && (
          <div className="fixes">
            <div className="eyebrow" style={{ padding: "4px 4px 0" }}>Fixes for this pull request</div>
            {fixes.map((fix) => <FixCard key={fix.id} fix={fix} now={now} />)}
          </div>
        )}
      </div>

      <div className="aside">
        {r.comment && (
          <div className="card" style={{ overflow: "hidden" }}>
            <div className="eyebrow" style={{ marginBottom: 10 }}>{r.posted ? "Comment posted to PR" : "Comment (not posted)"}</div>
            <div className="comment-head">
              <span className="logo" style={{ width: 22, height: 22, fontSize: 11, borderRadius: 7, background: "var(--accent)", color: "#fff" }}>W</span>
              <b>Warden</b><span className="tone-faint">via your token</span>
            </div>
            <div className="comment-text"><Markdown text={r.comment.replace(/@<[^>]+>/g, `@${r.pr.author}`)} /></div>
          </div>
        )}
        <div className="card">
          <div className="eyebrow" style={{ marginBottom: 4 }}>Timeline</div>
          <div>
            {timeline.map((e) => (
              <div className="timeline-row" key={e.label}>
                <i className="dot" style={{ width: 6, height: 6, background: e.tone ? `var(--${e.tone})` : "var(--faint)" }} />
                <span className="label">{e.label}</span>
                <span className={`mono ${e.tone ? `tone-${e.tone}` : ""}`}>{e.value}</span>
              </div>
            ))}
          </div>
        </div>
        {reviews.length > 1 && (
          <div className="card stack" style={{ gap: 4, alignItems: "stretch" }}>
            <div className="eyebrow">Reviews of this pull request</div>
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
          <details className="card">
            <summary className="eyebrow" style={{ cursor: "pointer" }}>Review log</summary>
            <div className="log mono" style={{ maxHeight: 260, borderRadius: 12, marginTop: 12, padding: 12, fontSize: 11 }}>
              {r.lines.map((l, i) => <div className="log-line" key={i}><span className="time">{l.time}</span><span className={`text k-${l.kind}`}>{l.text}</span></div>)}
            </div>
          </details>
        )}
      </div>
    </div>
    </>
  );
}
