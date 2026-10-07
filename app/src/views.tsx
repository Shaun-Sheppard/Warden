import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { HistoryRecord, Live, Settings } from "./types";
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
                  <span className={current.done ? "tone-muted" : ""} style={current.done ? undefined : { color: "var(--accent-fg)" }}>
                    {current.done ? "Review complete" : "Reviewing now"}
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
                  {STEPS.map((label, k) => (
                    <div key={label} className={`step ${k < current.step ? "done" : k === current.step ? "now" : ""}`}>
                      <div className="bar" /><span className="ellipsis">{label}</span>
                    </div>
                  ))}
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

export function History({ rows, now, onOpen }: { rows: HistoryRecord[]; now: number; onOpen: (recordId: string) => void }) {
  return (
    <div>
      <div className="history-head section-label mono">
        <span>Decision</span><span>Pull request</span><span>Raised by</span><span>Issues</span><span style={{ textAlign: "right" }}>Reviewed</span>
      </div>
      {rows.map((r) => (
        <button className="history-row" key={r.recordId} onClick={() => onOpen(r.recordId)}>
          <div className="stack">
            <span className={`pill tone-${tone(r)} tint-${tone(r)}`}>{decisionLabel(r)}</span>
            <span className="count">{outcomeLabel(r)}</span>
          </div>
          <div className="stack">
            <span className="ellipsis" style={{ fontWeight: 500, maxWidth: "100%" }}>{r.pr.title}</span>
            <span className="count mono ellipsis" style={{ maxWidth: "100%" }}>
              #{r.pr.id} · {r.pr.project}/{r.pr.repo}{r.stats ? ` · +${r.stats.additions} −${r.stats.deletions}` : ""}
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
      {rows.length === 0 && <div className="empty">No reviews match.</div>}
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

export function Detail({ record: r, onRetry, onSettings }: { record: HistoryRecord; onRetry: () => void; onSettings: () => void }) {
  const t = tone(r);
  const first = r.pr.author.split(" ")[0];
  const blocking = r.counts.critical + r.counts.major;
  const criteria = r.review?.criteria ?? [];
  const unmet = criteria.filter((c) => c.status === "not_met").length;
  const criterionTone = { met: "ok", not_met: "critical", unclear: "major" } as const;
  const criterionLabel = { met: "Met", not_met: "Not met", unclear: "Unclear" } as const;
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
  const groups = SEVERITIES.map((s) => ({ ...s, issues: (r.review?.comments ?? []).filter((c) => c.severity === s.key) })).filter((g) => g.issues.length);

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
        <div className={`headline tint-${t}`}>
          <b className={`tone-${t}`}>{headline}{r.dryRun && r.status !== "failed" ? " (dry run)" : ""}</b>
          <span className="tone-muted">{subline}</span>
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
          <div className="group">
            <div className="group-title">Acceptance criteria
              {criteria.length > 0 && <span className="count mono">{criteria.filter((c) => c.status === "met").length} of {criteria.length} met</span>}
            </div>
            {(r.workItems ?? []).length > 0 && (
              <div className="help selectable">Checked against {r.workItems.map((w) => `${w.kind} ${w.id}: ${w.title}`).join(" · ")}</div>
            )}
            {criteria.length === 0 && (
              <span className="help">
                {(r.workItems ?? []).length === 0 ? "No work items are linked to this pull request, so there was nothing to check it against." : "The linked work items have no acceptance criteria."}
              </span>
            )}
            {criteria.map((c, i) => (
              <div className="card issue" key={i}>
                <div className="row-between" style={{ gap: 16 }}>
                  <span className="selectable" style={{ fontWeight: 500 }}>{c.criterion}</span>
                  <span className={`pill tone-${criterionTone[c.status]} tint-${criterionTone[c.status]}`} style={{ flex: "none" }}>{criterionLabel[c.status]}</span>
                </div>
                {(c.note || c.work_item) && <div className="issue-body selectable">{c.work_item ? `#${c.work_item} · ` : ""}{c.note}</div>}
              </div>
            ))}
          </div>
        )}
        {r.review && groups.length === 0 && <span className="tone-ok">No issues found.</span>}
        {groups.map((g) => (
          <div className="group" key={g.key}>
            <div className="group-title"><i className="dot" style={{ background: `var(--${g.key})` }} />{g.label}<span className="count mono">{g.issues.length}</span></div>
            {g.issues.map((issue, i) => (
              <div className="card issue" key={i}>
                <div className="row-between" style={{ gap: 16 }}>
                  <span style={{ fontWeight: 500 }} className="selectable">{issue.title ?? issue.body.split("\n")[0]}</span>
                  <span className="mono" style={{ fontSize: 11, color: "var(--accent-fg)", flex: "none" }} title={issue.file ?? undefined}>{location(issue.file, issue.line)}</span>
                </div>
                {issue.title && <div className="issue-body selectable">{issue.body}</div>}
                {issue.snippet && <pre className="snippet mono">{issue.snippet}</pre>}
              </div>
            ))}
          </div>
        ))}
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
                <span className="mono logo" style={{ width: 20, height: 20, fontSize: 10, borderWidth: 1.5, borderRadius: 5 }}>W</span>
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
