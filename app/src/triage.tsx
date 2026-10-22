import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { api } from "./api";
import { ApproveAnyway } from "./approve";
import { DiffView } from "./fixes";
import type { ItemFix, TriageRecord, TriageState } from "./types";
import { ago } from "./util";

/** Where an item sits on the board. */
type Column = "triage" | "ai" | "person" | "fixing" | "review" | "failedReview" | "fixed";
type Tab = "all" | "ai" | "person" | "triage" | "prs";
type View = "list" | "board";

const COLUMNS: Record<Column, { label: string; desc: string; empty: string }> = {
  triage: { label: "To triage", desc: "Not yet sorted", empty: "Nothing to triage" },
  ai: { label: "AI can fix", desc: "Ready for Claude", empty: "Nothing here" },
  person: { label: "For a person", desc: "Needs a human", empty: "Nothing here" },
  fixing: { label: "Fixing", desc: "Claude, or you, working on these now", empty: "Nothing being fixed" },
  review: { label: "In review", desc: "Pull request waiting for approval", empty: "No pull requests in review" },
  failedReview: { label: "Failed review", desc: "Pull request sent back", empty: "Nothing sent back" },
  fixed: { label: "Fixed", desc: "Pull request approved", empty: "Nothing approved yet" },
};
const BOARD: Column[] = ["triage", "ai", "person", "fixing", "review", "failedReview", "fixed"];
const LIST: Column[] = ["fixing", "failedReview", "review", "triage", "ai", "person", "fixed"];
const TABS: { key: Tab; label: string; columns: Column[] }[] = [
  { key: "all", label: "All", columns: LIST },
  { key: "ai", label: "AI can fix", columns: ["fixing", "ai"] },
  { key: "person", label: "For a person", columns: ["person"] },
  { key: "triage", label: "To triage", columns: ["triage"] },
  { key: "prs", label: "Pull requests", columns: ["failedReview", "review", "fixed"] },
];

/** Cards shown per board column before "Show all". */
const COLUMN_LIMIT = 50;

const RUNNING: ItemFix["status"][] = ["fixing", "testing", "pushing"];
const BUSY: ItemFix["status"][] = ["queued", ...RUNNING];

function columnOf(r: TriageRecord): Column {
  const fix = r.fix;
  if (fix && RUNNING.includes(fix.status)) return "fixing";
  // Someone (you) moved it to in progress on the board.
  if (r.status === "left" && r.elsewhere && !fix) return "fixing";
  if (fix?.prId && fix.status !== "queued") {
    if (fix.review === "approved" || fix.review === "completed") return "fixed";
    if (fix.review === "rejected" || fix.review === "waiting" || fix.review === "abandoned" || fix.status === "failed") return "failedReview";
    return "review";
  }
  if (r.status !== "done" && r.status !== "left") return "triage";
  return r.recommendation === "ai" ? "ai" : "person";
}

/** Columns listed newest first, by when each item entered them. */
const NEWEST_FIRST: Column[] = ["fixing", "review", "failedReview", "fixed"];

/** When an item entered the column it is in now (0 if unknown). */
function enteredAt(r: TriageRecord): number {
  const fix = r.fix;
  const time = (value?: string | null) => (value ? Date.parse(value) || 0 : 0);
  if (fix && RUNNING.includes(fix.status)) return time(fix.startedAt);
  if (r.status === "left" && r.elsewhere && !fix?.prId) return time(r.elsewhereSince) || time(r.triagedAt);
  if (fix?.prId) return time(fix.reviewChangedAt) || time(fix.finishedAt) || time(fix.startedAt);
  return time(r.triagedAt);
}

/** A few words on why an item is for a person: Claude's own tag, or what can be told from the verdict. */
function reasonTag(r: TriageRecord): string | null {
  if (r.recommendation !== "human") return null;
  if (r.tag) return r.tag;
  if (r.manipulation) return "Suspicious text";
  if (!r.repo) return "No repo mapped";
  if (r.aiSuitable === false) return "Out of AI scope";
  if (r.confidence && r.confidence !== "high") return `${r.confidence[0].toUpperCase()}${r.confidence.slice(1)} confidence`;
  return null;
}

/** What a fix is doing now, in a few words. */
function fixLabel(fix: ItemFix): string {
  if (fix.feedback && RUNNING.includes(fix.status)) return "Addressing comments";
  return { queued: "Waiting to fix", fixing: "Fixing", testing: "Running tests", pushing: "Opening pull request", done: "Pull request open", dryRun: "Fixed · dry run", failed: "Fix failed" }[fix.status];
}

function Pill({ tone = "neutral", spin = false, children, title, onClick }: {
  tone?: "neutral" | "ok" | "major" | "critical" | "blue" | "purple" | "sand"; spin?: boolean; children: ReactNode; title?: string; onClick?: () => void;
}) {
  const body = <>{spin && <span className="pill-spin" aria-hidden />}{children}</>;
  return onClick
    ? <button className={`tpill tpill-${tone}`} title={title} onClick={(e) => { e.stopPropagation(); onClick(); }}>{body}</button>
    : <span className={`tpill tpill-${tone}`} title={title}>{body}</span>;
}

const RISK_TONE = { low: "ok", medium: "major", high: "critical" } as const;

const REVIEW = {
  open: { label: "in review", tone: "purple" },
  approved: { label: "approved", tone: "ok" },
  completed: { label: "merged", tone: "ok" },
  rejected: { label: "rejected", tone: "critical" },
  waiting: { label: "waiting for author", tone: "major" },
  abandoned: { label: "abandoned", tone: "neutral" },
} as const;

/** The pills shown on a card or row: what is happening, why, and the risk. */
function ItemPills({ r }: { r: TriageRecord }) {
  const fix = r.fix;
  const reason = reasonTag(r);
  return (
    <>
      {r.status === "triaging" && <Pill tone="blue" spin>Triaging</Pill>}
      {r.status === "left" && r.elsewhere && !fix && <Pill tone="sand" title="Moved to in progress on the board, outside Warden">By hand · {r.elsewhere}</Pill>}
      {r.status === "pending" && <Pill>Waiting</Pill>}
      {r.status === "failed" && <Pill tone="critical">Triage failed</Pill>}
      {fix && RUNNING.includes(fix.status) && <Pill tone="blue" spin>{fixLabel(fix)}</Pill>}
      {fix?.status === "queued" && <Pill>{fixLabel(fix)}</Pill>}
      {fix?.status === "failed" && <Pill tone="critical">{fix.prId ? `PR #${fix.prId} · last attempt failed` : "Fix failed"}</Pill>}
      {fix?.status === "dryRun" && <Pill tone="ok">Fixed · dry run</Pill>}
      {fix?.prId && fix.status === "done" && (
        <Pill tone={REVIEW[fix.review ?? "open"].tone} onClick={fix.prUrl ? () => api.open(fix.prUrl!) : undefined} title="Open the pull request">
          PR #{fix.prId} · {REVIEW[fix.review ?? "open"].label}
        </Pill>
      )}
      {reason && <Pill tone="sand">{reason}</Pill>}
      {r.screenedBy && <Pill title={`Screened by ${r.screenedBy} on this computer; Claude did not triage it`}>Screened locally</Pill>}
      {r.triagedBy && !r.screenedBy && <Pill title={`Triaged by ${r.triagedBy} on this computer, from the item's text`}>Triaged locally</Pill>}
      {r.overridden && <Pill title="You moved this item; Triage again lets Claude decide">Moved by you</Pill>}
      {!!r.history?.length && !r.fix && <Pill tone="major" title="Fixed before, and back on your board">Came back</Pill>}
      {r.complexity === "complex" && r.recommendation === "ai" && <Pill tone="blue" title="Fixed with the complex-fix model">Complex fix</Pill>}
      {r.risk && <Pill tone={RISK_TONE[r.risk]}>{r.risk} risk</Pill>}
    </>
  );
}

/** "↳ Product Backlog Item 4120 · Large refund: hold it for an admin…" for a task under a bigger item. */
function ParentLine({ r }: { r: TriageRecord }) {
  if (!r.parent) return null;
  return (
    <span className="parent-line" title={`${r.parent.kind} ${r.parent.id}: ${r.parent.title}`}>
      <span className="tone-faint">↳ {r.parent.kind} {r.parent.id} · </span>{r.parent.title}
    </span>
  );
}

function meta(r: TriageRecord) {
  return [r.kind, r.project, r.priority !== null ? `P${r.priority}` : null, r.repo ? `${r.repo} (${r.branch})` : null].filter(Boolean).join(" · ");
}

/** Claude's report, split into the parts it was asked for. Older free-text reports come back whole. */
export function parseReport(report: string) {
  const labels: [RegExp, string][] = [
    [/^summary\s*:/i, "summary"], [/^(wrong|what was wrong)\s*:/i, "What was wrong"], [/^(changed|what (i )?changed)\s*:/i, "What changed"],
    [/^tests? added\s*:/i, "Tests added"], [/^not checked\s*:/i, "notChecked"],
  ];
  const parts: { label: string; text: string }[] = [];
  const rest: string[] = [];
  for (const raw of report.split("\n")) {
    const line = raw.trim().replace(/^\*\*|\*\*(?=\s*:)/g, "");
    const hit = labels.find(([re]) => re.test(line));
    if (hit) parts.push({ label: hit[1], text: line.replace(hit[0], "").trim() });
    else if (parts.length && line) parts[parts.length - 1].text += ` ${line}`;
    else if (line) rest.push(raw);
  }
  const notChecked = parts.find((p) => p.label === "notChecked")?.text;
  return {
    summary: parts.find((p) => p.label === "summary")?.text ?? null,
    sections: parts.filter((p) => p.label !== "summary" && p.label !== "notChecked"),
    notChecked: notChecked && !/^(nothing|none)\.?$/i.test(notChecked) ? notChecked : null,
    rest: rest.join("\n").trim(),
  };
}

/** The four steps a fix goes through, and how far this item has got. */
function Steps({ r }: { r: TriageRecord }) {
  const fix = r.fix;
  const labels = ["Triaged", "Fixed", "Tests pass", "Pull request", "Approved"];
  let at = r.status === "done" || r.status === "left" ? 1 : 0;
  let failedAt = -1;
  if (fix) {
    const review = fix.review ?? "open";
    if (fix.status === "fixing") at = fix.prId ? 3 : 1;
    else if (fix.status === "testing") at = 2;
    else if (fix.status === "pushing") at = 3;
    else if (fix.status === "dryRun") at = 3;
    else if (fix.prId && (review === "approved" || review === "completed")) at = 5;
    else if (fix.prId && ["rejected", "waiting", "abandoned"].includes(review)) { failedAt = 4; at = 4; }
    else if (fix.prId) at = 4;
    else if (fix.status === "failed") {
      failedAt = fix.tests && !fix.tests.passed ? 2 : fix.commit ? 3 : 1;
      at = failedAt;
    }
  }
  const waiting = !!fix?.prId && (fix.review ?? "open") === "open" && fix.status === "done";
  const running = (fix && RUNNING.includes(fix.status)) || waiting;
  return (
    <div className="steps">
      {labels.map((label, n) => {
        const state = n === failedAt ? "failed" : n < at ? "done" : n === at && running ? "current" : "todo";
        return <div className={`step step-${state}`} key={label}><span className="step-bar" /><span className="step-label">{label}</span></div>;
      })}
    </div>
  );
}

function ReviewCallout({ fix }: { fix: ItemFix }) {
  const review = fix.review ?? "open";
  const checked = fix.reviewCheckedAt ? ` · checked ${ago(fix.reviewCheckedAt, Date.now())}` : "";
  if (review === "approved" || review === "completed") {
    return (
      <div className="callout callout-ok"><span className="callout-mark">✓</span>
        <span>Pull request {fix.prId} was {review === "completed" ? "approved and merged" : "approved"}{fix.movedToReview ? `, and the item was moved to ${fix.movedToReview}` : ""}.<span className="help">{checked}</span></span>
      </div>
    );
  }
  if (review === "rejected" || review === "waiting" || review === "abandoned") {
    return (
      <div className="callout callout-critical"><span className="callout-mark">!</span>
        <span>Pull request {fix.prId} {review === "abandoned" ? "was abandoned" : review === "rejected" ? "was rejected" : "is waiting for the author"}.
          {review !== "abandoned" ? " Choose Address PR comments to have Claude work through the review comments." : ""}<span className="help">{checked}</span></span>
      </div>
    );
  }
  return (
    <div className="callout callout-purple">
      <span>Pull request {fix.prId} is in review{fix.movedTo ? ` and the item is ${fix.movedTo}` : ""}. Once the pull request is approved, the item moves to In Review on the board.<span className="help">{checked}</span></span>
    </div>
  );
}

function FixSection({ fix, id }: { fix: ItemFix; id: number }) {
  const running = RUNNING.includes(fix.status);
  const report = parseReport(fix.report);
  const [diffOpen, setDiffOpen] = useState(false);
  // The change itself is not sent with the board (it can be large); it is fetched when opened.
  const [diff, setDiff] = useState<string | null>(null);
  useEffect(() => {
    if (!diffOpen) return;
    let live = true;
    setDiff(null);
    api.fixDiff(id).then((d) => live && setDiff(d), () => live && setDiff(""));
    return () => { live = false; };
  }, [diffOpen, id, fix.commit, fix.finishedAt]);
  const [logOpen, setLogOpen] = useState(running);
  const logRef = useRef<HTMLDivElement>(null);
  useEffect(() => { if (running) setLogOpen(true); }, [running]);
  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [fix.lines.length, logOpen]);
  const added = fix.files.reduce((n, f) => n + f.additions, 0);
  const removed = fix.files.reduce((n, f) => n + f.deletions, 0);
  const succeeded = fix.status === "done" || fix.status === "dryRun";
  const headline = report.summary ?? (fix.status === "dryRun" ? "Claude finished the fix." : "Claude finished the fix and it was pushed.");
  const first = fix.lines[0]?.time;
  const last = fix.lines[fix.lines.length - 1]?.time;

  return (
    <section className="modal-section">
      <div className="modal-section-head">
        <span className="eyebrow">Fix · {fix.feedback ? "PR comments" : fix.automatic ? "automatic" : "by hand"}{fix.model ? ` · ${fix.model.replace("@", " · ").replace(/^ · /, "")}` : ""}</span>
        {fix.branch && <span className="mono branch selectable">{fix.branch} → {fix.target}</span>}
      </div>

      {fix.notes && (
        <div className="approach"><b>Your notes for Claude</b><p className="modal-text selectable" style={{ whiteSpace: "pre-wrap" }}>{fix.notes}</p></div>
      )}
      {fix.status === "queued" && (
        <div className="callout callout-neutral">{fix.feedback ? "Addressing the PR comments starts" : "Starts"} as soon as fewer fixes are running than Settings allows.</div>
      )}
      {running && (
        <div className="callout callout-blue"><span className="pill-spin" aria-hidden />{fixLabel(fix)}… the run log below shows what Claude is doing.</div>
      )}
      {fix.status === "failed" && fix.error && (
        <div className="callout callout-critical"><span className="callout-mark">!</span><span className="selectable" style={{ whiteSpace: "pre-wrap" }}>{fix.error}</span></div>
      )}
      {succeeded && headline && (
        <div className="callout callout-ok"><span className="callout-mark">✓</span>
          <span className="selectable">{headline}{fix.tests?.passed ? " Build and tests pass." : ""}</span>
        </div>
      )}
      {fix.status === "dryRun" && (
        <div className="callout callout-neutral">Triage dry run is on, so nothing was committed, pushed or opened. Turn it off and choose “Fix again” to raise the pull request.</div>
      )}
      {fix.prUrl && <ReviewCallout fix={fix} />}
      {fix.warning && <div className="callout callout-major"><span className="callout-mark">!</span><span className="selectable">{fix.warning}</span></div>}

      {report.sections.length > 0 && (
        <div className="result-grid">
          {report.sections.map((s) => <Fragment key={s.label}><div className="result-label">{s.label}</div><div className="selectable">{s.text}</div></Fragment>)}
        </div>
      )}
      {report.sections.length === 0 && report.rest && <p className="modal-text selectable" style={{ whiteSpace: "pre-wrap" }}>{report.rest}</p>}
      {report.notChecked && (
        <div className="callout callout-major"><span className="callout-mark">!</span><span className="selectable"><b>Not checked:</b> {report.notChecked}</span></div>
      )}

      {fix.feedbackRounds.map((round, i) => (
        <div className="round" key={i}>
          <b>Review comments, round {i + 1}</b>
          <span className="help"> · {round.threads} thread{round.threads === 1 ? "" : "s"}{round.commit ? ` · pushed ${round.commit.slice(0, 8)}` : " · not pushed (dry run)"}</span>
          <div className="selectable tone-muted" style={{ whiteSpace: "pre-wrap", marginTop: 4 }}>{round.report}</div>
        </div>
      ))}

      <div className="disclosures">
        {fix.files.length > 0 && (
          <>
            <button className="disclosure" onClick={() => setDiffOpen(!diffOpen)} aria-expanded={diffOpen}>
              <span className={`chev ${diffOpen ? "open" : ""}`}>›</span>
              <span style={{ fontWeight: 500 }}>{fix.files.length} file{fix.files.length === 1 ? "" : "s"} changed</span>
              <span className="mono tone-ok">+{added}</span><span className="mono tone-critical">−{removed}</span>
            </button>
            {diffOpen && <div className="disclosure-body">{diff === null ? <span className="help"><span className="spinner" aria-hidden /> Loading the change…</span> : <DiffView fix={{ ...fix, diff }} />}</div>}
          </>
        )}
        <button className="disclosure" onClick={() => setLogOpen(!logOpen)} aria-expanded={logOpen}>
          <span className={`chev ${logOpen ? "open" : ""}`}>›</span>
          <span style={{ fontWeight: 500 }}>Run log</span>
          <span className="help">{fix.lines.length} step{fix.lines.length === 1 ? "" : "s"}{first && last ? ` · ${first}–${last}` : ""}</span>
        </button>
        {logOpen && (
          <div className="run-log mono selectable" ref={logRef}>
            {fix.lines.map((l, i) => <div className="run-log-line" key={i}><span className="tone-faint">{l.time}</span><span className={`k-${l.kind}`}>{l.text}</span></div>)}
            {fix.tests && (
              <div className={`run-log-line ${fix.tests.passed ? "tone-ok" : "tone-critical"}`}>
                <span>tests</span><span>{fix.tests.command || "(no command reported)"} · {fix.tests.passed ? "pass" : "do not pass"}, as reported by Claude</span>
              </div>
            )}
            {running && <div className="run-log-line"><span /><span className="cursor" /></div>}
          </div>
        )}
      </div>
    </section>
  );
}

/** The item's comments from Azure DevOps, newest first: what QA or others said, say, when it came back. */
function Comments({ id }: { id: number }) {
  const [comments, setComments] = useState<{ author: string; date: string; text: string }[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [all, setAll] = useState(false);
  useEffect(() => {
    let live = true;
    setComments(null);
    setError(null);
    api.itemComments(id).then((found) => live && setComments(found), (e) => live && setError(String(e)));
    return () => { live = false; };
  }, [id]);
  const shown = comments ? (all ? comments : comments.slice(0, 5)) : [];
  return (
    <section className="modal-section">
      <div className="modal-section-head">
        <span className="eyebrow">Comments{comments ? ` · ${comments.length}` : ""}</span>
        {comments && comments.length > 0 && <span className="help">Newest first</span>}
      </div>
      {!comments && !error && <span className="help"><span className="spinner" aria-hidden /> Loading comments…</span>}
      {error && <span className="help tone-critical">Could not load the comments: {error}</span>}
      {comments?.length === 0 && <span className="help">No comments on this item.</span>}
      {shown.map((c, i) => (
        <div className="comment" key={i}>
          <div className="comment-head"><b>{c.author}</b><span className="help">{c.date}</span></div>
          <div className="selectable comment-body">{c.text}</div>
        </div>
      ))}
      {comments && comments.length > 5 && (
        <button className="link" style={{ alignSelf: "flex-start" }} onClick={() => setAll(!all)}>{all ? "Show the latest 5" : `Show all ${comments.length}`}</button>
      )}
    </section>
  );
}

const MODAL_SIZE_KEY = "warden.triage.modalSize";

function readModalSize(): { width?: string; height?: string } | null {
  try {
    const saved = JSON.parse(sessionStorage.getItem(MODAL_SIZE_KEY) ?? "null");
    return saved && (saved.width || saved.height) ? saved : null;
  } catch {
    return null;
  }
}

function saveModalSize(size: { width: string; height: string }) {
  try { sessionStorage.setItem(MODAL_SIZE_KEY, JSON.stringify(size)); } catch { /* not kept */ }
}

/** The pop-up for one item: triage notes, the fix, and everything that can be done with it. */
function ItemModal({ r, onClose }: { r: TriageRecord; onClose: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The size you drag the pop-up to is kept for the next item, until Warden restarts.
  const modalRef = useRef<HTMLDivElement>(null);
  const [savedSize] = useState(readModalSize);
  useEffect(() => {
    const el = modalRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(() => {
      // Only a drag sets the size on the element itself; growing with its content does not.
      if (el.style.width || el.style.height) saveModalSize({ width: el.style.width, height: el.style.height });
    });
    watch.observe(el);
    return () => watch.disconnect();
  }, []);
  // Starting a fix first asks for anything Claude should know.
  const [composing, setComposing] = useState(false);
  const [notes, setNotes] = useState("");
  const fix = r.fix;
  const fixBusy = !!fix && BUSY.includes(fix.status);
  // Any triaged item with a repository can be fixed by hand; one already with a pull request cannot.
  // A failed fix can be tried again wherever the item is on the board.
  const canFix = (r.status === "done" || (r.status === "left" && fix?.status === "failed")) && !!r.repo && !fixBusy && !fix?.prId;
  const canAddress = !!fix?.prId && !fixBusy;
  const act = (run: () => Promise<unknown>) => { setBusy(true); setError(null); run().catch((e) => setError(String(e))).finally(() => setBusy(false)); };
  const column = columnOf(r);
  const groupTone = column === "fixed" ? "ok" : column === "failedReview" ? "critical" : column === "review" ? "purple"
    : r.recommendation === "ai" ? "ok" : r.recommendation === "human" ? "major" : "neutral";
  const groupLabel = ["fixed", "failedReview", "review"].includes(column) ? COLUMNS[column].label
    : r.recommendation === "ai" ? "AI can fix" : r.recommendation === "human" ? "For a person" : "To triage";
  const triaged = r.status === "done" || r.status === "left";

  return (
    <div className="modal-scrim" onClick={onClose}>
      <div className="modal" role="dialog" aria-modal="true" aria-label={r.title} onClick={(e) => e.stopPropagation()}
        ref={modalRef} style={savedSize ?? undefined}
        onMouseUp={(e) => { const el = e.currentTarget; if (el.style.width || el.style.height) saveModalSize({ width: el.style.width, height: el.style.height }); }}>
        <div className="modal-head">
          <div className="modal-title-row">
            <div className="stack" style={{ gap: 6, minWidth: 0 }}>
              <div className="modal-title selectable">{r.title} <span className="tone-faint" style={{ fontWeight: 400 }}>#{r.id}</span></div>
              <ParentLine r={r} />
              <div className="mono item-meta">{meta(r)}{r.status === "left" ? " · no longer on your board" : ""}</div>
            </div>
            <button className="modal-close" onClick={onClose} aria-label="Close">×</button>
          </div>
          <div className="pills">
            <ItemPills r={r} />
            <Pill tone={groupTone}>{groupLabel}</Pill>
          </div>
          {(triaged || fix) && <Steps r={r} />}
        </div>

        <div className="modal-body">
          <section className="modal-section">
            <span className="eyebrow">Triage</span>
            {r.status === "left" && r.elsewhere && !fix && (
              <div className="callout callout-neutral">Being fixed by hand: it is {r.elsewhere} on the board, moved there outside Warden. Warden leaves it alone, and shows it here until it moves on.</div>
            )}
            {!triaged && !r.error && <p className="modal-text tone-muted">{r.status === "triaging" ? "Claude is reading this item now." : "Triage notes will appear here once Claude has looked at this item."}</p>}
            {r.summary && <p className="modal-lead selectable">{r.summary}</p>}
            {r.reason && <p className="modal-text tone-muted selectable">{r.reason}</p>}
            {r.approach && r.recommendation === "ai" && (
              <div className="approach"><b>Approach</b><p className="modal-text selectable">{r.approach}</p></div>
            )}
            {r.files.length > 0 && (
              <div className="chips">{r.files.map((f) => <span className="chip mono" key={f} title={f}>{f.split("/").pop()}</span>)}</div>
            )}
            {r.triagedBy && !r.screenedBy && (
              <div className="callout callout-neutral">Triaged by {r.triagedBy} on this computer, from the item's text alone (it cannot read the code). Turn off “Use the local model for all triage” and choose “Triage again” to have Claude look at it.</div>
            )}
            {r.screenedBy && (
              <div className="callout callout-neutral">Screened by {r.screenedBy} on this computer, which judged it needs a person, so Claude did not triage it. Choose “Triage again” to have Claude look at it.</div>
            )}
            {r.manipulation && <div className="callout callout-critical"><span className="callout-mark">!</span><span className="selectable">Text aimed at automated tools was found and ignored: “{r.manipulation}”</span></div>}
            {r.error && <div className="callout callout-critical"><span className="callout-mark">!</span><span className="selectable" style={{ whiteSpace: "pre-wrap" }}>{r.error}</span></div>}
            {r.aiSuitable !== null && (
              <span className="help">
                AI suitable: {r.aiSuitable ? "yes" : "no"} · confidence {r.confidence}{r.complexity ? ` · ${r.complexity} fix` : ""}
                {r.tagged ? " · tagged" : ""}{r.commented ? " · you were mentioned in a comment" : ""}
                {r.triagedAt ? ` · triaged ${ago(r.triagedAt, Date.now())}` : ""}
              </span>
            )}
          </section>
          {fix && <FixSection fix={fix} id={r.id} />}
          <Comments id={r.id} />
          {!!r.history?.length && (
            <section className="modal-section">
              <span className="eyebrow">Earlier fixes</span>
              <p className="modal-text tone-muted">This item was fixed before and came back to your board, so it was triaged again with its latest comments.</p>
              {[...r.history].reverse().map((old, i) => {
                const summary = parseReport(old.report).summary;
                return (
                  <div className="round" key={i}>
                    <b>{old.prId ? `Pull request ${old.prId}` : "An earlier attempt"}</b>
                    <span className="help"> · {old.branch}{old.finishedAt ? ` · ${ago(old.finishedAt, Date.now())}` : ""}</span>
                    {summary && <div className="selectable tone-muted" style={{ marginTop: 4 }}>{summary}</div>}
                    {old.prUrl && <div style={{ marginTop: 4 }}><button className="link" onClick={() => api.open(old.prUrl!)}>Open pull request ↗</button></div>}
                  </div>
                );
              })}
            </section>
          )}
        </div>

        {composing && (
          <div className="fix-notes">
            <label className="field">
              <span>Anything Claude should know? <span className="tone-faint" style={{ fontWeight: 400 }}>· optional</span></span>
              <textarea className="input" rows={4} value={notes} autoFocus spellCheck
                placeholder={r.recommendation === "human"
                  ? "e.g. the business rule is that withdrawn subjects never count as active; the totals come from ReportService"
                  : "e.g. where the problem is, how it should behave, or what to leave alone"}
                onChange={(e) => setNotes(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { setComposing(false); act(() => api.fixWorkItem(r.id, notes)); } }} />
              <span className="help">Passed to Claude as your guidance, alongside the work item, its comments and the triage notes.</span>
            </label>
            <div className="inline">
              <button className="btn primary" disabled={busy} onClick={() => { setComposing(false); act(() => api.fixWorkItem(r.id, notes)); }}>Start fix</button>
              <button className="btn ghost" onClick={() => setComposing(false)}>Cancel</button>
              <span className="help">⌘↩ to start</span>
            </div>
          </div>
        )}
        <div className="modal-foot">
          {canFix && !composing && (
            <button className={`btn ${r.recommendation === "ai" ? "primary" : ""}`} disabled={busy}
              title={r.recommendation === "human" ? "Triage thought a person should handle this. Claude fixes it anyway; the tests must pass and Warden reviews the pull request." : undefined}
              onClick={() => { setNotes(fix?.notes ?? ""); setComposing(true); }}>
              {fix ? "Fix again" : r.recommendation === "human" ? "Fix anyway" : "Fix with Claude"}
            </button>
          )}
          {canAddress && (
            <button className="btn primary" disabled={busy} title="Claude reads the open comments on the pull request, changes the code, runs the tests and pushes to the same branch"
              onClick={() => act(() => api.addressPrFeedback(r.id))}>Address PR comments</button>
          )}
          {triaged && !fixBusy && !fix?.prId && r.recommendation === "human" && (
            <button className="btn" disabled={busy} title="Put it with the items an AI agent can fix. With Fix automatically on, it is queued for Claude."
              onClick={() => act(() => api.setRecommendation(r.id, true))}>Move to AI can fix</button>
          )}
          {triaged && !fixBusy && !fix?.prId && r.recommendation === "ai" && (
            <button className="btn ghost" disabled={busy} title="Leave it for a person; automatic fixes skip it"
              onClick={() => act(() => api.setRecommendation(r.id, false))}>{r.overridden ? "Move back to For a person" : "Move to For a person"}</button>
          )}
          {triaged && !fixBusy && !fix?.prId && r.repo && (
            <button className="btn ghost" disabled={busy} title="Which fix model this item gets"
              onClick={() => act(() => api.setComplexity(r.id, r.complexity !== "complex"))}>
              {r.complexity === "complex" ? "Treat as a simple fix" : "Treat as a complex fix"}
            </button>
          )}
          {column === "failedReview" && fix?.prId && (fix.review === "rejected" || fix.review === "waiting") && !fixBusy && (
            <ApproveAnyway prId={fix.prId} project={r.project} compact />
          )}
          {fix?.prUrl && <button className="btn" onClick={() => api.open(fix.prUrl!)}>Open pull request ↗</button>}
          <button className="btn" onClick={() => api.open(r.url)}>Open in Azure DevOps ↗</button>
          {r.status !== "triaging" && r.status !== "pending" && !fixBusy && (
            <button className="btn ghost" disabled={busy} onClick={() => act(() => api.triageAgain(r.id))}>Triage again</button>
          )}
          {r.status !== "triaging" && !(fix && RUNNING.includes(fix.status)) && (
            <button className="btn ghost" disabled={busy}
              title="Forget this item. The next check adds it back, triaged afresh, if it is still assigned to you in a to-do state."
              onClick={() => act(() => api.forgetItem(r.id).then(onClose))}>Remove from Warden</button>
          )}
          {error && <span className="tone-critical">{error}</span>}
          <span className="help" style={{ marginLeft: "auto" }}>Esc to close</span>
        </div>
      </div>
    </div>
  );
}

/** What can be done with an item from the board without opening it. */
interface ItemActions {
  open: (id: number) => void;
  newSession: (r: TriageRecord) => void;
  remove: (r: TriageRecord) => void;
}

/** The ⋯ button on a card or row, and its menu. */
function ItemMenu({ r, actions }: { r: TriageRecord; actions: ItemActions }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); setOpen(false); } };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", key, true);
    return () => { document.removeEventListener("mousedown", away); document.removeEventListener("keydown", key, true); };
  }, [open]);
  const column = columnOf(r);
  // Only items still waiting for someone to start on them.
  const canSession = (column === "ai" || column === "person") && !!r.repo;
  const pick = (run: () => void) => (e: React.MouseEvent) => { e.stopPropagation(); setOpen(false); run(); };
  return (
    <div className="item-menu" ref={ref} onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.stopPropagation()}>
      <button className="item-menu-button" aria-label={`Actions for #${r.id}`} aria-haspopup="menu" aria-expanded={open}
        onClick={(e) => { e.stopPropagation(); setOpen(!open); }}>⋯</button>
      {open && (
        <div className="item-menu-list" role="menu">
          <button role="menuitem" onClick={pick(() => actions.open(r.id))}>Open</button>
          {canSession && (
            <button role="menuitem" onClick={pick(() => actions.newSession(r))}
              title={`A new worktree from the latest base branch, and a Claude Code session there with /fix ${r.id} ready`}>New session</button>
          )}
          <button role="menuitem" className="danger" onClick={pick(() => actions.remove(r))}
            title="Forget this item; the next check adds it back if it still qualifies">Remove</button>
        </div>
      )}
    </div>
  );
}

/** Opens an item from the keyboard as well as by click (cards are not buttons, as they hold one). */
const activate = (open: () => void) => ({
  role: "button" as const,
  tabIndex: 0,
  onClick: open,
  onKeyDown: (e: React.KeyboardEvent) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); } },
});

function Card({ r, actions }: { r: TriageRecord; actions: ItemActions }) {
  const column = columnOf(r);
  return (
    <div className={`board-card ${column === "fixing" ? "is-fixing" : ""} ${column === "fixed" ? "is-fixed" : ""}`} {...activate(() => actions.open(r.id))}>
      <span className="card-head">
        <span className="card-title">{r.title} <span className="tone-faint" style={{ fontWeight: 400 }}>#{r.id}</span></span>
        <ItemMenu r={r} actions={actions} />
      </span>
      <ParentLine r={r} />
      <span className="mono item-meta">{meta(r)}</span>
      <span className="pills"><ItemPills r={r} /></span>
    </div>
  );
}

function Row({ r, actions, first }: { r: TriageRecord; actions: ItemActions; first: boolean }) {
  return (
    <div className={`list-row ${first ? "" : "divided"}`} {...activate(() => actions.open(r.id))}>
      <span className="stack" style={{ gap: 4, minWidth: 0 }}>
        <span className="row-title">{r.title} <span className="tone-faint" style={{ fontWeight: 400 }}>#{r.id}</span></span>
        <ParentLine r={r} />
        <span className="mono item-meta">{meta(r)}</span>
      </span>
      <span className="pills" style={{ justifyContent: "flex-end" }}><ItemPills r={r} /></span>
      <ItemMenu r={r} actions={actions} />
    </div>
  );
}

function storedView(): View {
  try { return localStorage.getItem("warden.triage.view") === "list" ? "list" : "board"; } catch { return "board"; }
}

export function Triage({ triage, now, autoFix, paused, onSettings, project, onProject }: {
  triage: TriageState; now: number; autoFix: boolean; paused: boolean; onSettings: () => void;
  /** The project shown ("" for all), shared with the sidebar. */
  project: string; onProject: (project: string) => void;
}) {
  const [tab, setTab] = useState<Tab>("all");
  const [view, setViewState] = useState<View>(storedView);
  const setView = (v: View) => { setViewState(v); try { localStorage.setItem("warden.triage.view", v); } catch { /* not kept */ } };
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Partial<Record<Column, boolean>>>({ fixed: true });

  const [openId, setOpenId] = useState<number | null>(null);
  const [showAll, setShowAll] = useState<Partial<Record<Column, boolean>>>({});
  // What happened after a board action (a new session, a removal), shown above the board.
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const actions: ItemActions = {
    open: (id) => setOpenId(id),
    newSession: (r) => {
      setNotice({ ok: true, text: `Getting a worktree ready for #${r.id}…` });
      api.newSession(r.id).then(
        (path) => setNotice({ ok: true, text: `Opened Claude in ${path}. Confirm the folder, then press Enter to start /fix ${r.id}.` }),
        (e) => setNotice({ ok: false, text: `Could not start a session for #${r.id}: ${e}` }),
      );
    },
    remove: (r) => {
      api.forgetItem(r.id).then(
        () => setNotice({ ok: true, text: `Removed #${r.id}. It comes back at the next check if it still qualifies.` }),
        (e) => setNotice({ ok: false, text: String(e) }),
      );
    },
  };
  const setProject = onProject;
  const [retrying, setRetrying] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setOpenId(null); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Items leave the board once their pull request moves them on; keep those in view.
  // Items leave the to-do states as soon as a fix moves them on, so being fixed or
  // having a pull request keeps them in view.
  const allRecords = triage.records.filter((r) => r.status !== "left" || !!r.fix?.prId || !!r.elsewhere
    || (!!r.fix && (BUSY.includes(r.fix.status) || r.fix.status === "failed")));
  const projects = [...new Set(allRecords.map((r) => r.project))].sort();
  // A remembered project that no longer has items shows everything.
  const inProject = project && projects.includes(project) ? project : "";
  const records = inProject ? allRecords.filter((r) => r.project === inProject) : allRecords;
  const onBoard = records.filter((r) => r.status !== "left").length;
  const failedFixes = records.filter((r) => r.fix?.status === "failed").length;
  const inFlight = triage.records.filter((r) => r.status === "triaging" || (r.fix && RUNNING.includes(r.fix.status))).length;
  const retry = () => { setRetrying(true); api.retryFailedFixes().finally(() => setRetrying(false)); };

  // A search looks through everything: by PR number, item number or title.
  const q = query.trim().replace(/^[#!]|^pr\s*#?/i, "");
  const number = /^\d+$/.test(q) ? Number(q) : null;
  const matches = (r: TriageRecord) => !q || (number !== null
    ? r.fix?.prId === number || r.id === number || r.parent?.id === number
    : `${r.title} ${r.parent?.title ?? ""}`.toLowerCase().includes(q.toLowerCase()));
  // A search looks in every project, so a PR number is always found.
  const sorted = (q ? allRecords : records)
    .filter(matches)
    .sort((a, b) => Number(b.status === "triaging") - Number(a.status === "triaging") || (a.priority ?? 99) - (b.priority ?? 99) || a.id - b.id);
  // Work in motion is newest first, by when it entered the column; the rest by priority.
  const inColumn = (c: Column) => {
    const items = sorted.filter((r) => columnOf(r) === c);
    return NEWEST_FIRST.includes(c) ? items.sort((a, b) => enteredAt(b) - enteredAt(a) || b.id - a.id) : items;
  };
  const count = (cs: Column[]) => records.filter((r) => cs.includes(columnOf(r))).length;

  // Finding one item by number opens it.
  const only = number !== null && sorted.length === 1 ? sorted[0].id : null;
  useEffect(() => { if (only !== null) setOpenId(only); }, [only]);

  const open = openId !== null ? triage.records.find((r) => r.id === openId) : undefined;
  const current = triage.records.find((r) => r.id === triage.current);
  const groups = (TABS.find((t) => t.key === tab) ?? TABS[0]).columns
    .map((c) => ({ c, items: inColumn(c) }))
    .filter((g) => !(q && g.items.length === 0));
  const anyOpen = groups.some((g) => !collapsed[g.c]);

  return (
    <div className={`triage ${view === "board" ? "is-board" : ""}`}>
      {triage.error && (
        <div className="warning tint-critical" role="alert">
          <b className="tone-critical">Triage could not run</b>
          <span className="selectable" style={{ whiteSpace: "pre-wrap" }}>{triage.error}</span>
        </div>
      )}
      {triage.fixProblem && (
        <div className="warning tint-critical" role="alert">
          <b className="tone-critical">Fixing is paused</b>
          <span className="selectable" style={{ whiteSpace: "pre-wrap" }}>{triage.fixProblem}</span>
          <span>Every fix would fail the same way, so none are started. Sort it out in <button className="link" onClick={onSettings}>Settings</button> (saving lifts the pause), then retry.</span>
          {failedFixes > 0 && <div><button className="btn" disabled={retrying} onClick={retry}>Retry failed fixes ({failedFixes})</button></div>}
        </div>
      )}

      {paused && (
        <div className="warning tint-major" role="status">
          <b className="tone-major">Monitoring is paused</b>
          <span>
            {inFlight > 0 ? `Finishing the ${inFlight} item${inFlight === 1 ? "" : "s"} already being worked on; nothing new starts` : "Nothing new is reviewed, triaged or fixed"} until
            you resume with the play button at the top of the sidebar. The board is still kept up to date.
          </span>
        </div>
      )}
      {notice && (
        <div className={`warning ${notice.ok ? "tint-ok" : "tint-critical"}`} role="status">
          <div className="inline" style={{ justifyContent: "space-between", alignItems: "flex-start" }}>
            <span className="selectable">{notice.text}</span>
            <button className="modal-close" style={{ width: 24, height: 24, fontSize: 14 }} onClick={() => setNotice(null)} aria-label="Dismiss">×</button>
          </div>
        </div>
      )}
      <div className="triage-bar">
        <div className="triage-bar-head">
          <b>{onBoard} item{onBoard === 1 ? "" : "s"} assigned to you in a to-do state{inProject ? ` in ${inProject}` : ""}</b>
          <span className="triage-bar-meta">
            <span className="help">
              {triage.lastCheck ? `Boards checked ${ago(triage.lastCheck, now)}` : ""}
            </span>
          </span>
        </div>
        {current && (
          <span className="help">
            <span className="spinner" aria-hidden /> Triaging #{current.id}{triage.currentModel ? <> with <b className="tone-muted">{triage.currentModel}</b></> : null}: {triage.lines[triage.lines.length - 1]?.text ?? current.title}
          </span>
        )}
        {(() => {
          const toTriage = triage.records.filter((r) => r.status === "pending" || r.status === "triaging").length;
          const ready = triage.records.filter((r) => r.status === "done" && r.recommendation === "ai" && (!r.fix || (r.fix.automatic && r.fix.status === "queued"))).length;
          return autoFix && toTriage > 0 && ready > 0
            ? <span className="help">Automatic fixes start once the remaining {toTriage} item{toTriage === 1 ? " is" : "s are"} triaged, so they can run side by side. “Fix with Claude” starts one straight away.</span>
            : null;
        })()}
        <div className="triage-bar-tools">
          <input className="triage-search" type="search" value={query} onChange={(e) => setQuery(e.target.value)}
            placeholder="Find PR number, item or title" aria-label="Find a triage item by pull request number, item number or title" />
          {view === "list" && (
            <div className="seg" role="tablist" aria-label="Show">
              {TABS.map((t) => (
                <button key={t.key} role="tab" aria-selected={tab === t.key} className={tab === t.key ? "on" : ""} onClick={() => setTab(t.key)}>
                  {t.label}{t.key !== "all" && <span className="seg-count">{count(t.columns)}</span>}
                </button>
              ))}
            </div>
          )}
          <span className="spacer" />
          {failedFixes > 0 && !triage.fixProblem && (
            <button className="btn" disabled={retrying} onClick={retry} title="Queue every failed fix again">Retry failed fixes ({failedFixes})</button>
          )}
          {view === "list" && groups.length > 0 && (
            <button className="btn ghost" onClick={() => setCollapsed(anyOpen ? Object.fromEntries(LIST.map((c) => [c, true])) : {})}>
              {anyOpen ? "Collapse all" : "Expand all"}
            </button>
          )}
          {projects.length > 1 && (
            <select className="triage-project" value={inProject} onChange={(e) => setProject(e.target.value)} aria-label="Project">
              <option value="">All projects</option>
              {projects.map((p) => <option key={p} value={p}>{p} ({allRecords.filter((r) => r.project === p && r.status !== "left").length})</option>)}
            </select>
          )}
          <div className="seg" role="tablist" aria-label="View">
            {(["list", "board"] as View[]).map((v) => (
              <button key={v} role="tab" aria-selected={view === v} className={view === v ? "on" : ""} onClick={() => setView(v)}>{v === "list" ? "List" : "Board"}</button>
            ))}
          </div>
        </div>
      </div>

      {records.length === 0 && !triage.error && (
        <div className="empty">
          <b>{triage.lastCheck ? "Nothing to triage" : "Waiting for the first check"}</b>
          <span>Warden looks for work items assigned to you in the states and types set in <button className="link" onClick={onSettings}>Settings</button>.</span>
        </div>
      )}
      {q && sorted.length === 0 && <div className="empty"><b>No match</b><span>Nothing in the triage list has that pull request number, item number or title.</span></div>}

      {records.length > 0 && view === "board" && (
        <div className="board">
          {BOARD.map((c) => {
            const items = inColumn(c);
            return (
              <div className="board-col" key={c}>
                <div className="col-head">
                  <div className="col-title"><span className={`dot dot-${c}`} /><b>{COLUMNS[c].label}</b><span className="col-count">{items.length}</span></div>
                  <span className="help col-desc">{COLUMNS[c].desc}</span>
                </div>
                {items.length === 0 && <div className="col-empty">{q ? "No matches" : COLUMNS[c].empty}</div>}
                {(showAll[c] ? items : items.slice(0, COLUMN_LIMIT)).map((r) => <Card key={r.id} r={r} actions={actions} />)}
                {items.length > COLUMN_LIMIT && (
                  <button className="col-more" onClick={() => setShowAll((s) => ({ ...s, [c]: !s[c] }))}>
                    {showAll[c] ? `Show the first ${COLUMN_LIMIT}` : `Show all ${items.length} (${items.length - COLUMN_LIMIT} more)`}
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}

      {records.length > 0 && view === "list" && groups.map(({ c, items }) => {
        const shut = !!collapsed[c];
        return (
          <section className="list-group" key={c}>
            <button className="group-head" onClick={() => setCollapsed((s) => ({ ...s, [c]: !s[c] }))} aria-expanded={!shut}>
              <span className={`chev ${shut ? "" : "open"}`}>›</span>
              <span className={`dot dot-${c}`} />
              <b>{COLUMNS[c].label}</b>
              <span className="col-count">{items.length}</span>
              <span className="help">{COLUMNS[c].desc}</span>
            </button>
            {!shut && (
              <div className={`list-card ${c === "fixing" ? "is-fixing" : ""}`}>
                {items.length === 0 && <div className="list-empty">{q ? "No matches." : `${COLUMNS[c].empty}.`}</div>}
                {items.map((r, i) => <Row key={r.id} r={r} first={i === 0} actions={actions} />)}
              </div>
            )}
          </section>
        );
      })}

      {open && <ItemModal r={open} onClose={() => setOpenId(null)} />}
    </div>
  );
}
