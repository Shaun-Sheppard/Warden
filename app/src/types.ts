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
  manipulation: string | null;
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
  /** Don't review a pull request again once it has been approved. */
  skipApproved?: boolean;
  setupComplete: boolean;
  theme: "system" | "light" | "dark";
  mentionAuthor: boolean;
  reviewPrompt: string;
  triagePrompt: string;
  projectConfigs: ProjectConfig[];
  /** Claude models per kind of work; "" for Claude Code's default. */
  reviewModel: string;
  triageModel: string;
  fixModelSimple: string;
  fixModelComplex: string;
  fixEscalate: boolean;
  /** Effort per kind of work: low, medium, high, xhigh or max; "" for the default. */
  reviewEffort?: string;
  triageEffort?: string;
  fixEffortSimple?: string;
  fixEffortComplex?: string;
  /** Screen items with a local model (Ollama) before Claude triage. */
  localPretriage?: boolean;
  localUrl?: string;
  localModel?: string;
  /** The local model does all of triage; Claude is not used for triage. */
  localTriageOnly?: boolean;
  /** Only items in each project's current sprint. */
  triageCurrentSprint?: boolean;
  triageDryRun: boolean;
  triageAutoFix: boolean;
  /** Triage and fixing start nothing new until resumed. */
  triagePaused?: boolean;
  triageMaxFixes: number;
  triagePostResults: boolean;
  notifications: boolean;
  /** Stop the computer going to sleep when idle while Warden runs. */
  keepAwake?: boolean;
  triageEnabled: boolean;
  triageProjects: string[];
  triageStates: string[];
  triageTypes: string[];
  areaMappings: AreaMapping[];
}

/** Everything configured for one project: its pull request reviews, and its board triage. */
export interface ProjectConfig {
  name: string;
  review: boolean;
  people: string[];
  approveAndComplete: boolean;
  mergeStrategy: string;
  deleteSourceBranch: boolean;
  triage: boolean;
  autoFix: boolean;
  currentSprint: boolean;
  postResults: boolean;
  states: string[];
  types: string[];
  areas: AreaMapping[];
}

export interface AreaMapping {
  project: string;
  areaPath: string;
  repo: string;
  branch: string;
  /** The user's own checkout, to copy local-only files from. */
  localPath: string;
  copyPaths: string[];
  copyLocalOnly: boolean;
  /** Runs once in each fix's worktree before Claude starts. */
  setupCommand: string;
}

export interface TriageRecord {
  id: number;
  project: string;
  kind: string;
  title: string;
  state: string;
  areaPath: string;
  priority: number | null;
  url: string;
  status: "pending" | "triaging" | "done" | "failed" | "left";
  risk: "low" | "medium" | "high" | null;
  aiSuitable: boolean | null;
  confidence: "low" | "medium" | "high" | null;
  recommendation: "ai" | "human" | null;
  summary: string;
  reason: string;
  approach: string;
  files: string[];
  repo: string | null;
  branch: string | null;
  manipulation: string | null;
  tagged: boolean;
  commented: boolean;
  error: string | null;
  attempts: number;
  triagedAt: string | null;
  durationSecs: number;
  fix: ItemFix | null;
  /** For an item that needs a person: a few words on why. */
  tag?: string;
  /** The user moved the item to another group than triage chose. */
  overridden?: boolean;
  /** How hard the fix looks: "simple" or "complex". */
  complexity?: string;
  /** The local model that screened this item as needing a person, when Claude did not triage it. */
  screenedBy?: string | null;
  /** The local model that did all of this item's triage. */
  triagedBy?: string | null;
  /** Earlier fixes, newest last, for an item that came back to the board. */
  history?: ItemFix[];
  /** The item this one sits under, such as the PBI a "Dev" task belongs to. */
  parent?: { id: number; kind: string; title: string } | null;
  /** Moved to the board's in-progress state by hand (not by Warden): that state. */
  elsewhere?: string | null;
  elsewhereSince?: string | null;
}

export type ItemFixStatus = "queued" | "fixing" | "testing" | "pushing" | "done" | "dryRun" | "failed";

export interface TestRun {
  command: string;
  passed: boolean;
  output: string;
  durationSecs: number;
}

export interface ItemFix {
  status: ItemFixStatus;
  automatic: boolean;
  /** The queued or running work addresses comments on the pull request. */
  feedback: boolean;
  /** What the user told Claude when starting the fix. */
  notes?: string;
  /** The model the fix ran on last. */
  model?: string;
  feedbackRounds: { at: string; threads: number; report: string; commit: string | null }[];
  branch: string;
  target: string;
  repo: string;
  baseCommit: string | null;
  commit: string | null;
  prId: number | null;
  prUrl: string | null;
  movedTo: string | null;
  /** Where the pull request's review stands. */
  review?: "open" | "approved" | "rejected" | "waiting" | "completed" | "abandoned" | null;
  reviewCheckedAt?: string | null;
  /** When the review status last changed: when the item entered its column. */
  reviewChangedAt?: string | null;
  /** The state the item was moved to once the pull request was approved. */
  movedToReview?: string | null;
  report: string;
  files: { path: string; additions: number; deletions: number }[];
  diff: string;
  diffTruncated: boolean;
  tests: TestRun | null;
  rounds: number;
  error: string | null;
  warning: string | null;
  startedAt: string;
  finishedAt: string | null;
  lines: LogLine[];
}

export interface TriageState {
  /** Fixing is paused: a fix failed on a settings or setup problem every fix would hit. */
  fixProblem?: string | null;
  records: TriageRecord[];
  current: number | null;
  /** The model working on the current item. */
  currentModel?: string | null;
  lines: LogLine[];
  lastCheck: string | null;
  error: string | null;
}

export interface TriageChoices {
  types: string[];
  states: string[];
}

export interface ProjectStructure {
  areas: string[];
  repos: { name: string; defaultBranch: string }[];
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
  /** Approved by the user despite the review, with their reason. */
  approvedAnyway?: string | null;
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

export interface Fix {
  id: string;
  recordId: string;
  pr: PrInfo;
  status: "generating" | "ready" | "pushing" | "pushed" | "noChanges" | "failed";
  issues: { severity: Severity; title: string; location: string; body: string }[];
  baseCommit: string | null;
  diff: string;
  diffTruncated: boolean;
  files: { path: string; additions: number; deletions: number }[];
  report: string;
  error: string | null;
  commit: string | null;
  startedAt: string;
  finishedAt: string | null;
  lines: LogLine[];
}

export interface Current {
  pr: PrInfo;
  step: number;
  startedAt: string;
  lines: LogLine[];
  done: boolean;
  recordId: string | null;
  failed: number[];
  skipped: number[];
}

export interface Live {
  /** When the next check is due, and when the wait for it began. */
  nextCheck?: string | null;
  waitStarted?: string | null;
  status: "setup" | "paused" | "reviewing" | "error" | "idle";
  checking: boolean;
  lastCheck: string | null;
  error: string | null;
  current: Current | null;
  queue: PrInfo[];
  claimed: { pr: PrInfo; by: string }[];
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
  supported: boolean;
  gitFound: boolean;
}

export interface Legacy {
  organization: string;
  projects: string[];
  people: string[];
  approveAndComplete: boolean;
}
