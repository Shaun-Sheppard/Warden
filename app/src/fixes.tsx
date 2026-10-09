import { useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { Fix } from "./types";
import { Avatar } from "./ui";
import { ago } from "./util";

interface DiffFile {
  path: string;
  lines: { kind: "add" | "del" | "hunk" | "ctx"; text: string }[];
}

/** Splits a unified diff into files, dropping git's own header lines. */
export function parseDiff(diff: string): DiffFile[] {
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  for (const line of diff.split("\n")) {
    if (line.startsWith("diff --git ")) {
      const match = / b\/(.*)$/.exec(line);
      current = { path: match ? match[1] : line.slice(11), lines: [] };
      files.push(current);
    } else if (!current || /^(index |--- |\+\+\+ |new file mode|deleted file mode|similarity index|rename (from|to) |old mode|new mode)/.test(line)) {
      continue;
    } else if (line.startsWith("@@")) {
      current.lines.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) {
      current.lines.push({ kind: "add", text: line });
    } else if (line.startsWith("-")) {
      current.lines.push({ kind: "del", text: line });
    } else {
      current.lines.push({ kind: "ctx", text: line });
    }
  }
  return files;
}

function DiffView({ fix }: { fix: Fix }) {
  const files = parseDiff(fix.diff);
  return (
    <div className="stack" style={{ gap: 10, alignItems: "stretch" }}>
      {files.map((file) => {
        const stat = fix.files.find((f) => f.path === file.path);
        return (
          <details className="diff-file" key={file.path} open>
            <summary>
              <span className="mono selectable">{file.path}</span>
              {stat && <span className="mono count"><span className="tone-ok">+{stat.additions}</span> <span className="tone-critical">−{stat.deletions}</span></span>}
            </summary>
            <pre className="diff mono">
              {file.lines.map((l, i) => <span key={i} className={`diff-line ${l.kind}`}>{l.text || " "}{"\n"}</span>)}
            </pre>
          </details>
        );
      })}
      {fix.diffTruncated && <span className="help">This change is too large to show in full. Everything Claude changed would be pushed, including what is not shown here.</span>}
    </div>
  );
}

const STATUS: Record<Fix["status"], { label: string; tone: string }> = {
  generating: { label: "Preparing", tone: "muted" },
  ready: { label: "Awaiting your review", tone: "major" },
  pushing: { label: "Pushing", tone: "muted" },
  pushed: { label: "Pushed", tone: "ok" },
  noChanges: { label: "No changes made", tone: "muted" },
  failed: { label: "Failed", tone: "critical" },
};

function FixCard({ fix, now }: { fix: Fix; now: number }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const logRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [fix.lines.length]);
  const act = (action: Promise<void>) => {
    setBusy(true);
    setError(null);
    action.then(() => setBusy(false), (e) => { setBusy(false); setError(String(e)); });
  };
  const status = STATUS[fix.status];
  const working = fix.status === "generating" || fix.status === "pushing";

  return (
    <section className="card section-card" aria-label={`Fix for pull request ${fix.pr.id}`}>
      <div className="section-head" style={{ alignItems: "flex-start" }}>
        <Avatar name={fix.pr.author} size={28} />
        <div className="stack" style={{ gap: 3, flex: 1 }}>
          <span className="section-title selectable">{fix.pr.title} <span className="mono tone-faint" style={{ fontWeight: 400 }}>#{fix.pr.id}</span></span>
          <span className="meta mono">{fix.pr.author} · {fix.pr.project}/{fix.pr.repo} · {fix.pr.source}</span>
        </div>
        <span className={`badge-soft ${status.tone === "muted" ? "" : `tone-${status.tone} tint-${status.tone}`}`}>
          {working && <span className="spinner" aria-hidden />}{status.label}
        </span>
        <span className="count mono" style={{ alignSelf: "center" }}>{ago(fix.finishedAt ?? fix.startedAt, now)}</span>
      </div>

      <div className="issue-row">
        <div className="section-label mono">Issues to fix</div>
        {fix.issues.map((issue, i) => (
          <div className="inline" key={i} style={{ alignItems: "baseline" }}>
            <i className="dot" style={{ width: 6, height: 6, background: `var(--${issue.severity})`, transform: "translateY(-1px)" }} />
            <span className="selectable" style={{ flex: 1 }}>{issue.title}</span>
            <span className="mono" style={{ fontSize: 11, color: "var(--accent-fg)" }}>{issue.location}</span>
          </div>
        ))}
      </div>

      {fix.status === "generating" && (
        <div className="log mono" ref={logRef} style={{ height: 180 }}>
          {fix.lines.map((l, i) => <div className="log-line" key={i}><span className="time">{l.time}</span><span className={`text k-${l.kind}`}>{l.text}</span></div>)}
          <div className="log-line"><span className="time" style={{ visibility: "hidden" }}>00:00:00</span><span className="cursor" /></div>
        </div>
      )}

      {fix.report && (
        <div className="issue-row">
          <div className="section-label mono">What Claude says it did</div>
          <div className="selectable tone-muted" style={{ whiteSpace: "pre-wrap", lineHeight: 1.55 }}>{fix.report}</div>
        </div>
      )}

      {fix.status === "ready" || fix.status === "pushing" ? (
        <>
          <div className="issue-row">
            <div className="row-between">
              <div className="section-label mono">Changes to review</div>
              <span className="count mono">{fix.files.length} file{fix.files.length === 1 ? "" : "s"}</span>
            </div>
            <DiffView fix={fix} />
          </div>
          <div className="issue-row">
            <span className="help">
              Warden cannot build or run tests, so these changes are unverified. Pushing commits them to <b className="mono">{fix.pr.source}</b> as you; Warden then reviews the pull request again.
            </span>
            {(error || fix.error) && <span className="tone-critical selectable" role="alert">{error || fix.error}</span>}
            <div className="inline">
              <button className="btn primary" disabled={busy || fix.status === "pushing"} onClick={() => act(api.pushFix(fix.id))}>
                {fix.status === "pushing" ? "Pushing…" : "Approve and push"}
              </button>
              <button className="btn" disabled={busy || fix.status === "pushing"} onClick={() => act(api.discardFix(fix.id))}>Discard</button>
            </div>
          </div>
        </>
      ) : fix.status !== "generating" && (
        <div className="issue-row">
          {fix.status === "pushed" && (
            <span>Pushed as <b className="mono">{fix.commit?.slice(0, 7)}</b> to <span className="mono">{fix.pr.source}</span>. Warden reviews the pull request again on its next check.</span>
          )}
          {fix.status === "noChanges" && <span className="tone-muted">Claude did not change anything, so there is nothing to push.</span>}
          {(error || fix.error) && <span className="tone-critical selectable" role="alert" style={{ whiteSpace: "pre-wrap" }}>{error || fix.error}</span>}
          <div className="inline">
            <button className="btn" onClick={() => api.open(fix.pr.url)}>Open in Azure DevOps ↗</button>
            <button className="btn ghost" disabled={busy} onClick={() => act(api.discardFix(fix.id))}>Remove from list</button>
          </div>
        </div>
      )}
    </section>
  );
}

export function Fixes({ fixes, now }: { fixes: Fix[]; now: number }) {
  if (fixes.length === 0) {
    return (
      <div className="empty">
        <b>No fixes yet</b>
        <span style={{ maxWidth: 460 }}>
          Open a review, tick the issues you want fixed and choose "Fix selected with Claude". The changes appear here for you to read, and nothing is pushed until you approve them.
        </span>
      </div>
    );
  }
  return <div className="fixes">{fixes.map((fix) => <FixCard key={fix.id} fix={fix} now={now} />)}</div>;
}
