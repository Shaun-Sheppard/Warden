import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
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
  onLive(handler: (live: Live) => void): () => void;
  onHistory(handler: (history: HistoryRecord[]) => void): () => void;
  onSettings(handler: (settings: Settings) => void): () => void;
}

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
  onLive: (handler) => subscribe("live", handler),
  onHistory: (handler) => subscribe("history", handler),
  onSettings: (handler) => subscribe("settings", handler),
};

// Outside the desktop shell (plain `npm run dev` in a browser) the UI runs
// against sample data, so screens can be worked on without Azure DevOps.
export const api: Api = "__TAURI_INTERNALS__" in window ? tauriApi : mockApi;
