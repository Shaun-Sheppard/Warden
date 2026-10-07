// Sample data and a scripted "live review" for working on the UI in a browser.
import type { Api } from "./api";
import type { HistoryRecord, Issue, Live, LogLine, PrInfo, Review, Settings } from "./types";

const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();

const pr = (id: number, title: string, project: string, repo: string, author: string, source: string): PrInfo => ({
  id, title, project, repo, repoId: repo, author, source, target: "main",
  url: `https://dev.azure.com/harbourline/${project}/_git/${repo}/pullrequest/${id}`, commit: "4c7e01b",
});

const issue = (severity: Issue["severity"], title: string, file: string, line: number, body: string, snippet: string | null = null): Issue =>
  ({ severity, title, file, line, body, snippet });

function record(p: PrInfo, mins: number, secs: number, review: Review, extra: Partial<HistoryRecord> = {}): HistoryRecord {
  const counts = { critical: 0, major: 0, minor: 0 };
  review.comments.forEach((c) => counts[c.severity]++);
  const approved = counts.critical + counts.major === 0 && review.verdict !== "changes_requested";
  return {
    recordId: `${p.id}-${mins}`, pr: p, status: approved ? "approved" : "rejected", dryRun: false, review, counts,
    comment: `🤖 AI-assisted review: @<guid> **Decision: ${approved ? "Approve" : "Reject"}**\n\n${review.summary}`,
    posted: true, vote: approved ? "approved" : "waitingForAuthor", autoComplete: approved, merged: approved && mins > 30,
    stats: { files: 9, additions: 412, deletions: 58 }, startedAt: minutesAgo(mins + 3), finishedAt: minutesAgo(mins),
    durationSecs: secs, error: null, lines: [], workItems: review.criteria.length ? [{ id: 5120, kind: "User Story", title: "Refunds are safe to retry" }] : [], ...extra,
  };
}

const live4821 = pr(4821, "Add idempotency keys to refund endpoint", "Payments", "payments-api", "Priya Nair", "feature/refund-idempotency");

let history: HistoryRecord[] = [
  record(pr(4817, "Migrate invoice PDF rendering to worker queue", "Payments", "payments-api", "Tom Okafor", "feature/invoice-worker"), 14, 221, {
    verdict: "changes_requested",
    summary: "Moves PDF generation into a background queue. Failed jobs are silently dropped and tenant branding is cached without isolation.",
    comments: [
      issue("critical", "Tenant branding cached across merchants", "src/Invoices/BrandingCache.cs", 22, "Cache key is the template name only. Two merchants using the default template share one entry."),
      issue("major", "Failed render jobs are acknowledged and lost", "src/Invoices/PdfWorker.cs", 77, "The message is completed before RenderAsync returns. Any exception after that point drops the job with no retry.", "await msg.CompleteAsync();\nvar pdf = await _renderer.RenderAsync(invoice);"),
      issue("minor", "Queue name hard-coded", "src/Invoices/PdfWorker.cs", 14, "Read from configuration so staging and production queues stay separate."),
    ],
    criteria: [],
  }),
  record(pr(4815, "Bump Serilog to 4.1 and tidy logging config", "Platform", "shared-libs", "Daniel Reyes", "chore/serilog-4.1"), 41, 72, {
    verdict: "approve", summary: "Dependency bump with config clean-up. No behavioural change beyond removing a duplicate console sink.", comments: [], criteria: [],
  }),
  record(pr(2290, "Lazy-load order history on account page", "Customer Web", "web-portal", "Aisha Rahman", "feature/lazy-orders"), 65, 128, {
    verdict: "approve_with_suggestions",
    summary: "Splits order history into its own chunk and loads it on scroll. Two small robustness gaps, nothing blocking.",
    comments: [
      issue("minor", "Loading state never clears on fetch error", "src/account/OrderHistory.tsx", 48, "Set isLoading to false in the catch branch, otherwise the spinner stays forever."),
      issue("minor", "IntersectionObserver not disconnected on unmount", "src/hooks/useInView.ts", 19, "Return observer.disconnect from the effect cleanup."),
    ],
    criteria: [],
  }, { vote: "approvedWithSuggestions" }),
  {
    ...record(pr(612, "Split staging network module", "Platform", "infra", "Marcus Lee", "feature/split-network"), 190, 12, { verdict: "approve", summary: "", comments: [], criteria: [] }),
    status: "failed", review: null, comment: null, posted: false, vote: null, autoComplete: false, merged: false, stats: null,
    error: "The `claude` CLI is not signed in (Failed to authenticate: OAuth session expired and could not be refreshed). Run `claude auth login` in a terminal, then try again.",
  },
];

const SCRIPT: [number, LogLine["kind"], string][] = [
  [0, "info", "PR 4821 by Priya Nair · feature/refund-idempotency → main"],
  [0, "cmd", "Fetching PR #4821 from Azure DevOps"],
  [1, "cmd", "Updating the local clone of payments-api"],
  [2, "cmd", "Claude is reviewing"],
  [2, "info", "Bash git diff origin/main...origin/feature/refund-idempotency"],
  [2, "info", "Read src/Refunds/RefundController.cs"],
  [2, "info", "Read src/Refunds/IdempotencyStore.cs"],
  [2, "info", "Grep IdempotencyKey"],
  [3, "major", "MAJOR    Idempotency key lookup not scoped to merchant  IdempotencyStore.cs:41"],
  [3, "minor", "MINOR    Key TTL hard-coded to 24 hours  IdempotencyStore.cs:18"],
  [3, "warn", "Decision → reject (0 critical, 1 major, 1 minor)"],
  [4, "info", "Comment posted · mentioned Priya Nair"],
  [5, "warn", "Vote set: Waiting for author · will review again when new commits are pushed"],
];

let settings: Settings = {
  organization: "harbourline", projects: ["Payments", "Customer Web", "Platform"],
  people: ["Priya Nair", "Tom Okafor", "Aisha Rahman"], pollSeconds: 60, approveAndComplete: true,
  mergeStrategy: "squash", deleteSourceBranch: true, cliPath: "", dryRun: false, paused: false,
  reviewExisting: false, setupComplete: !location.search.includes("setup"), theme: "system", mentionAuthor: true, reviewPrompt: "",
};

let tick = location.search.includes("idle") ? -1 : 5;
const startedAt = new Date().toISOString();
let checking = false;
let lastCheck = minutesAgo(0.2);
const liveHandlers = new Set<(live: Live) => void>();
const historyHandlers = new Set<(h: HistoryRecord[]) => void>();
const now = () => new Date().toTimeString().slice(0, 8);

function live(): Live {
  if (tick < 0) {
    const failing = location.search.includes("error");
    return {
      status: !settings.setupComplete ? "setup" : settings.paused ? "paused" : failing ? "error" : "idle",
      lastCheck, checking, queue: [], current: null,
      error: failing ? "Azure DevOps rejected the Personal Access Token (HTTP 401). It is invalid or expired." : null,
    };
  }
  const shown = SCRIPT.slice(0, Math.min(tick, SCRIPT.length));
  const done = tick > SCRIPT.length;
  return {
    status: !settings.setupComplete ? "setup" : done ? (settings.paused ? "paused" : "idle") : "reviewing",
    lastCheck, checking, error: null,
    queue: [pr(2293, "Add skeleton loaders to account page", "Customer Web", "web-portal", "Aisha Rahman", "feature/account-skeletons")],
    current: {
      pr: live4821, step: done ? 6 : shown[shown.length - 1]?.[0] ?? 0, startedAt, done,
      recordId: done ? "4821-live" : null,
      lines: shown.map(([, kind, text]) => ({ time: now(), kind, text })),
    },
  };
}

setInterval(() => {
  if (tick < 0 || tick > SCRIPT.length) return;
  tick++;
  if (tick > SCRIPT.length) {
    history = [{
      ...record(live4821, 0, 172, {
        verdict: "changes_requested",
        summary: "Adds an Idempotency-Key header to the refund endpoint. The cache isn't partitioned by merchant, so it can return another merchant's refund.",
        comments: [
          issue("major", "Idempotency key lookup not scoped to merchant", "src/Refunds/IdempotencyStore.cs", 41, "Keys are global, so two merchants sending the same key receive each other's cached refund response.", "var cached = await _db.Keys\n  .FirstOrDefaultAsync(k => k.Value == key);"),
          issue("minor", "Key TTL hard-coded to 24 hours", "src/Refunds/IdempotencyStore.cs", 18, "Move to options so it can match the API documentation."),
        ],
        criteria: [
          { work_item: 5120, criterion: "A repeated request with the same key returns the original response", status: "met", note: "Implemented in IdempotencyStore.GetOrAdd." },
          { work_item: 5120, criterion: "Keys are isolated per merchant", status: "not_met", note: "Lookup is by key only; merchant id is not part of the query." },
          { work_item: 5120, criterion: "Keys expire after the documented period", status: "unclear", note: "TTL is 24h in code; the documented period is not in the repository." },
        ],
      }),
      recordId: "4821-live", lines: live().current?.lines ?? [],
    }, ...history];
    historyHandlers.forEach((h) => h(history));
  }
  liveHandlers.forEach((h) => h(live()));
}, 1400);

const later = <T,>(value: T, ms = 150) => new Promise<T>((resolve) => setTimeout(() => resolve(value), ms));

export const mockApi: Api = {
  getSettings: () => later(settings),
  saveSettings: (s) => { settings = s; liveHandlers.forEach((h) => h(live())); return later(s); },
  getLive: () => later(live()),
  getHistory: () => later(history),
  checkNow: () => {
    checking = true;
    liveHandlers.forEach((h) => h(live()));
    setTimeout(() => {
      checking = false;
      lastCheck = new Date().toISOString();
      liveHandlers.forEach((h) => h(live()));
    }, 600);
    return later(undefined);
  },
  retryReview: () => later(undefined),
  applyVote: (recordId) => {
    history = history.map((r) => (r.recordId === recordId ? { ...r, vote: r.status === "approved" ? "approved" : "waitingForAuthor", autoComplete: r.status === "approved" } : r));
    historyHandlers.forEach((h) => h(history));
    return later(undefined, 600);
  },
  hasPat: () => later(settings.setupComplete),
  testConnection: (organization) =>
    organization ? later({ user: "Shaun Sheppard", projects: ["Payments", "Customer Web", "Platform", "Internal Tools"], projectsNote: null }, 700)
      : Promise.reject("Enter your organization first."),
  listPeople: () => later(["Aisha Rahman", "Daniel Reyes", "Ellie Shaw", "Marcus Lee", "Priya Nair", "Tom Okafor"]),
  claudeStatus: () => later({ found: true, path: "/opt/homebrew/bin/claude", version: "2.1.291 (Claude Code)", signedIn: true, gitFound: true }),
  importLegacy: () => later(null),
  defaultPrompt: () => later("You are an experienced engineer reviewing a pull request.\n\nReview guidance:\n- First, check the change against the acceptance criteria of the linked work items.\n- Then look for defects the change introduces.\n- Look specifically for security problems the change introduces."),
  open: (url) => { window.open(url, "_blank"); return later(undefined); },
  onLive: (h) => { liveHandlers.add(h); return () => liveHandlers.delete(h); },
  onHistory: (h) => { historyHandlers.add(h); return () => historyHandlers.delete(h); },
  onSettings: () => () => {},
};
