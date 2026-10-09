import type { HistoryRecord, Severity } from "./types";

export const SEVERITIES: { key: Severity; label: string }[] = [
  { key: "critical", label: "Critical" },
  { key: "major", label: "Major" },
  { key: "minor", label: "Minor" },
];

export const STEPS = ["Detected", "Cloned", "Claude review", "Decision", "Comment", "Vote & merge"];

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  return (parts.length > 1 ? parts[0][0] + parts[parts.length - 1][0] : (parts[0] ?? "?").slice(0, 2)).toUpperCase();
}

/** A stable colour per person. */
export function avatarColor(name: string): string {
  let hash = 0;
  for (const ch of name) hash = (hash * 31 + ch.charCodeAt(0)) % 360;
  return `linear-gradient(135deg, oklch(0.66 0.13 ${hash}), oklch(0.5 0.14 ${hash + 35}))`;
}

export function ago(iso: string | null, now: number): string {
  if (!iso) return "never";
  const secs = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
  if (secs < 5) return "just now";
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  if (secs < 172800) return "Yesterday";
  return new Date(iso).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}

export function duration(secs: number): string {
  return secs >= 60 ? `${Math.floor(secs / 60)}m ${String(secs % 60).padStart(2, "0")}s` : `${secs}s`;
}

export function clock(secs: number): string {
  return `${String(Math.floor(secs / 60)).padStart(2, "0")}:${String(secs % 60).padStart(2, "0")}`;
}

export function pollLabel(secs: number): string {
  return secs < 60 ? `${secs}s` : `${Math.round(secs / 60)}m`;
}

export function countsLabel(r: HistoryRecord, short = false): string {
  const c = r.counts;
  return short
    ? `${c.critical} critical · ${c.major} major · ${c.minor} minor`
    : `${c.critical} critical, ${c.major} major, ${c.minor} minor`;
}

export function decisionLabel(r: HistoryRecord): string {
  return r.status === "approved" ? "Approved" : r.status === "rejected" ? "Rejected" : "Failed";
}

/** What actually happened on Azure DevOps as a result of the review. */
export function outcomeLabel(r: HistoryRecord): string {
  if (r.status === "failed") return "Review failed";
  if (r.dryRun) return "Dry run · nothing posted";
  if (r.merged) return "Merged";
  if (r.autoComplete) return "Auto-complete set";
  if (r.vote === "waitingForAuthor") return "Waiting on author";
  if (r.vote) return "Approved";
  if (r.posted) return "Commented · no vote";
  return "Nothing posted";
}

export function location(file: string | null, line: number | null): string {
  if (!file) return "general";
  const name = file.split("/").pop() ?? file;
  return line ? `${name}:${line}` : name;
}
