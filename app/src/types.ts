export type Severity = "critical" | "major" | "minor";

export interface Issue {
  file: string | null;
  line: number | null;
  severity: Severity;
  title: string | null;
  body: string;
  snippet: string | null;
}

export interface Criterion {
  work_item: number | null;
  criterion: string;
  status: "met" | "not_met" | "unclear";
  note: string | null;
}

export interface Review {
  verdict: "approve" | "approve_with_suggestions" | "changes_requested";
  summary: string;
  comments: Issue[];
  criteria: Criterion[];
}

export interface Settings {
  organization: string;
  projects: string[];
  people: string[];
  pollSeconds: number;
  approveAndComplete: boolean;
  mergeStrategy: "squash" | "noFastForward" | "rebase";
  deleteSourceBranch: boolean;
  cliPath: string;
  dryRun: boolean;
  paused: boolean;
  reviewExisting: boolean;
  setupComplete: boolean;
  theme: "system" | "light" | "dark";
  mentionAuthor: boolean;
  reviewPrompt: string;
}

export interface PrInfo {
  id: number;
  title: string;
  project: string;
  repo: string;
  repoId: string;
  author: string;
  source: string;
  target: string;
  url: string;
  commit: string | null;
}

export interface LogLine {
  time: string;
  kind: "info" | "cmd" | "dim" | "ok" | "warn" | Severity;
  text: string;
}

export type Outcome = "approved" | "rejected" | "failed" | "baseline";

export interface HistoryRecord {
  recordId: string;
  pr: PrInfo;
  status: Outcome;
  dryRun: boolean;
  review: Review | null;
  counts: Record<Severity, number>;
  comment: string | null;
  posted: boolean;
  vote: "approved" | "approvedWithSuggestions" | "waitingForAuthor" | null;
  autoComplete: boolean;
  merged: boolean;
  stats: { files: number; additions: number; deletions: number } | null;
  startedAt: string;
  finishedAt: string;
  durationSecs: number;
  error: string | null;
  lines: LogLine[];
  workItems: { id: number; kind: string; title: string }[];
}

export interface Current {
  pr: PrInfo;
  step: number;
  startedAt: string;
  lines: LogLine[];
  done: boolean;
  recordId: string | null;
}

export interface Live {
  status: "setup" | "paused" | "reviewing" | "error" | "idle";
  checking: boolean;
  lastCheck: string | null;
  error: string | null;
  current: Current | null;
  queue: PrInfo[];
}

export interface Connection {
  user: string;
  projects: string[];
  projectsNote: string | null;
}

export interface ClaudeStatus {
  found: boolean;
  path: string;
  version: string;
  signedIn: boolean;
  gitFound: boolean;
}

export interface Legacy {
  organization: string;
  projects: string[];
  people: string[];
  approveAndComplete: boolean;
}
