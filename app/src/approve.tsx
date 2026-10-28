import { useState } from "react";
import { api } from "./api";

/** "Approve anyway" for a pull request that failed review: the user gives a reason, which is posted
 *  as a reply on Warden's review comment (resolving it), then the PR is approved and set to auto-complete. */
export function ApproveAnyway({ prId, project, compact = false, inline = false, onDone }: {
  prId: number; project: string; compact?: boolean;
  /** A reason field and button on one line, always shown. */
  inline?: boolean; onDone?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ready = reason.trim().length >= 5;
  const submit = () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    api.approveAnyway(prId, project, reason.trim()).then(
      () => { setBusy(false); setOpen(false); setReason(""); onDone?.(); },
      (e) => { setBusy(false); setError(String(e)); },
    );
  };

  if (inline) {
    return (
      <div className="stack" style={{ gap: 6, alignItems: "stretch" }}>
        <div className="approve-anyway inline-form">
          <input className="input" value={reason} spellCheck placeholder="Reason, e.g. the flagged query only runs in the nightly export"
            aria-label="Why it is fine to approve it" onChange={(e) => setReason(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") submit(); }} />
          <button className="btn" disabled={busy || !ready} onClick={submit}
            title="Posted on the pull request as a reply to Warden's review comment, which is then resolved; the pull request is approved in your name and set to auto-complete">
            {busy ? "Approving…" : "Approve anyway"}
          </button>
        </div>
        {error && <span className="tone-critical selectable" role="alert">{error}</span>}
      </div>
    );
  }
  if (!open) {
    return (
      <button className="btn" onClick={() => setOpen(true)}
        title="Approve this pull request despite the review, with a reason that is posted on it">
        Approve anyway
      </button>
    );
  }
  return (
    <div className={`approve-anyway ${compact ? "compact" : ""}`}>
      <label className="field">
        <span>Why is it fine to approve it?</span>
        <textarea className="input" rows={3} value={reason} autoFocus spellCheck
          placeholder="e.g. The flagged query is only used by the nightly export, which is already rate-limited."
          onChange={(e) => setReason(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit(); }} />
        <span className="help">
          Posted on the pull request as a reply to Warden's review comment, which is then resolved. The pull request is approved
          in your name and set to auto-complete with the project's merge settings.
        </span>
      </label>
      <div className="inline">
        <button className="btn primary" disabled={busy || !ready} onClick={submit}>{busy ? "Approving…" : "Approve and set auto-complete"}</button>
        <button className="btn ghost" disabled={busy} onClick={() => { setOpen(false); setError(null); }}>Cancel</button>
      </div>
      {error && <span className="tone-critical selectable" role="alert">{error}</span>}
    </div>
  );
}
