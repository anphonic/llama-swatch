import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Settings, SettingsView, Snapshot, TestResult } from "./types";

export const getSettings = () => invoke<SettingsView>("get_settings");

/** apiKey: null keeps the saved key, "" removes it, anything else replaces it. */
export const saveSettings = (settings: Settings, apiKey: string | null) =>
  invoke<SettingsView>("save_settings", { settings, apiKey });

export const testConnection = (baseUrl: string, apiKey: string | null) =>
  invoke<TestResult>("test_connection", { baseUrl, apiKey });

export const getSnapshot = () => invoke<Snapshot | null>("get_snapshot");

export const onSnapshot = (cb: (s: Snapshot) => void): Promise<UnlistenFn> =>
  listen<Snapshot>("snapshot", (e) => cb(e.payload));
