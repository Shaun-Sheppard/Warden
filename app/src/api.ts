import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { sendNotification } from "@tauri-apps/plugin-notification";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { mockApi } from "./mock";
import type { ClaudeStatus, Connection, HistoryRecord, Legacy, Live, Settings } from "./types";

export interface Api {
  getSettings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<Settings>;
  getLive(): Promise<Live>;
  getHistory(): Promise<HistoryRecord[]>;
  checkNow(): Promise<void>;
  retryReview(prId: number): Promise<void>;
  applyVote(recordId: string): Promise<void>;
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
  /** Downloads and installs the update found by `checkForUpdate`, then restarts. */
  installUpdate(onProgress: (percent: number | null) => void): Promise<void>;
  onLive(handler: (live: Live) => void): () => void;
  onHistory(handler: (history: HistoryRecord[]) => void): () => void;
  onSettings(handler: (settings: Settings) => void): () => void;
}

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
  installUpdate: async (onProgress) => {
    if (!pendingUpdate) throw new Error("No update is ready to install.");
    let total = 0;
    let received = 0;
    await pendingUpdate.downloadAndInstall((event) => {
      if (event.event === "Started") total = event.data.contentLength ?? 0;
      if (event.event === "Progress") {
        received += event.data.chunkLength;
        onProgress(total ? Math.min(100, Math.round((received / total) * 100)) : null);
      }
    });
    await relaunch();
  },
  onLive: (handler) => subscribe("live", handler),
  onHistory: (handler) => subscribe("history", handler),
  onSettings: (handler) => subscribe("settings", handler),
};

// Outside the desktop shell (plain `npm run dev` in a browser) the UI runs
// against sample data, so screens can be worked on without Azure DevOps.
export const api: Api = "__TAURI_INTERNALS__" in window ? tauriApi : mockApi;
