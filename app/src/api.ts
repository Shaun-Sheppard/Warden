import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { mockApi } from "./mock";
import type { ClaudeStatus, Connection, Fix, HistoryRecord, Legacy, Live, Settings } from "./types";

export interface Api {
  getSettings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<Settings>;
  getLive(): Promise<Live>;
  getHistory(): Promise<HistoryRecord[]>;
  checkNow(): Promise<void>;
  retryReview(prId: number): Promise<void>;
  applyVote(recordId: string): Promise<void>;
  getFixes(): Promise<Fix[]>;
  /** Has Claude prepare a fix for the chosen issues (by position in the review). Pushes nothing. */
  startFix(recordId: string, issues: number[]): Promise<string>;
  /** Commits and pushes a fix the user has approved. */
  pushFix(fixId: string): Promise<void>;
  discardFix(fixId: string): Promise<void>;
  onFixes(handler: (fixes: Fix[]) => void): () => void;
  hasPat(): Promise<boolean>;
  testConnection(organization: string, pat: string | null): Promise<Connection>;
  listPeople(organization: string, projects: string[]): Promise<string[]>;
  claudeStatus(): Promise<ClaudeStatus>;
  importLegacy(): Promise<Legacy | null>;
  defaultPrompt(): Promise<string>;
  open(url: string): Promise<void>;
  appVersion(): Promise<string>;
  /** Shows a system notification. */
  notify(title: string, body: string): void;
  /** The newer release, if there is one. */
  checkForUpdate(): Promise<AvailableUpdate | null>;
  /**
   * Downloads the update found by `checkForUpdate`, waits for any review or
   * fix in flight to finish, then installs it and restarts.
   */
  installUpdate(onStage: (stage: UpdateStage) => void): Promise<void>;
  onLive(handler: (live: Live) => void): () => void;
  onHistory(handler: (history: HistoryRecord[]) => void): () => void;
  onSettings(handler: (settings: Settings) => void): () => void;
}

export type UpdateStage = { stage: "downloading"; percent: number | null } | { stage: "waiting" } | { stage: "installing" };

export interface AvailableUpdate {
  version: string;
  notes: string;
}

let pendingUpdate: Update | null = null;

function subscribe<T>(event: string, handler: (payload: T) => void): () => void {
  const pending = listen<T>(event, (e) => handler(e.payload));
  return () => {
    pending.then((unlisten) => unlisten());
  };
}

const tauriApi: Api = {
  getSettings: () => invoke("get_settings"),
  saveSettings: (settings) => invoke("save_settings", { settings }),
  getLive: () => invoke("get_live"),
  getHistory: () => invoke("get_history"),
  checkNow: () => invoke("check_now"),
  retryReview: (prId) => invoke("retry_review", { prId }),
  applyVote: (recordId) => invoke("apply_vote", { recordId }),
  getFixes: () => invoke("get_fixes"),
  startFix: (recordId, issues) => invoke("start_fix", { recordId, issues }),
  pushFix: (fixId) => invoke("push_fix", { fixId }),
  discardFix: (fixId) => invoke("discard_fix", { fixId }),
  onFixes: (handler) => subscribe("fixes", handler),
  hasPat: () => invoke("has_pat"),
  testConnection: (organization, pat) => invoke("test_connection", { organization, pat }),
  listPeople: (organization, projects) => invoke("list_people", { organization, projects }),
  claudeStatus: () => invoke("claude_status"),
  importLegacy: () => invoke("import_legacy"),
  defaultPrompt: () => invoke("default_prompt"),
  open: (url) => openUrl(url),
  appVersion: () => getVersion(),
  notify: (title, body) => sendNotification({ title, body }),
  checkForUpdate: async () => {
    pendingUpdate = await check();
    return pendingUpdate ? { version: pendingUpdate.version, notes: pendingUpdate.body ?? "" } : null;
  },
  installUpdate: async (onStage) => {
    if (!pendingUpdate) throw new Error("No update is ready to install.");
    // From here no new review or fix starts, so the restart cannot cut one off.
    await invoke("begin_update");
    try {
      let total = 0;
      let received = 0;
      onStage({ stage: "downloading", percent: null });
      await pendingUpdate.download((event) => {
        if (event.event === "Started") total = event.data.contentLength ?? 0;
        if (event.event === "Progress") {
          received += event.data.chunkLength;
          onStage({ stage: "downloading", percent: total ? Math.min(100, Math.round((received / total) * 100)) : null });
        }
      });
      // Anything already running is allowed to finish first.
      while (await invoke<boolean>("is_busy")) {
        onStage({ stage: "waiting" });
        await new Promise((resolve) => setTimeout(resolve, 2000));
      }
      onStage({ stage: "installing" });
      await pendingUpdate.install();
      await relaunch();
    } catch (e) {
      await invoke("end_update");
      throw e;
    }
  },
  onLive: (handler) => subscribe("live", handler),
  onHistory: (handler) => subscribe("history", handler),
  onSettings: (handler) => subscribe("settings", handler),
};

// Outside the desktop shell (plain `npm run dev` in a browser) the UI runs
// against sample data, so screens can be worked on without Azure DevOps.
export const api: Api = "__TAURI_INTERNALS__" in window ? tauriApi : mockApi;
