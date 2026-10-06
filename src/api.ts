import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ActivityRow, ColorScheme, Settings, SettingsView, Snapshot, TestResult } from "./types";

export const getSettings = () => invoke<SettingsView>("get_settings");

/** apiKey: null keeps the saved key, "" removes it, anything else replaces it. */
export const saveSettings = (settings: Settings, apiKey: string | null) =>
  invoke<SettingsView>("save_settings", { settings, apiKey });

export const testConnection = (baseUrl: string, apiKey: string | null) =>
  invoke<TestResult>("test_connection", { baseUrl, apiKey });

/** Whether `baseUrl` normalizes (backend rules) to the saved URL, i.e. would reuse the saved key. */
export const isSavedUrl = (baseUrl: string) => invoke<boolean>("is_saved_url", { baseUrl });

/** Remembers the pin button's state; the window itself is changed with `setAlwaysOnTop`. */
export const setAlwaysOnTopSetting = (on: boolean) => invoke<void>("set_always_on_top", { on });

export const getSnapshot = () => invoke<Snapshot | null>("get_snapshot");

export const getActivity = (limit: number) => invoke<ActivityRow[]>("get_activity", { limit });

/** Resolves when llama-swap has the model up (can take minutes); rejects with a short message. */
export const loadModel = (id: string) => invoke<void>("load_model", { id });

export const unloadModel = (id: string) => invoke<void>("unload_model", { id });

export const onSnapshot = (cb: (s: Snapshot) => void): Promise<UnlistenFn> =>
  listen<Snapshot>("snapshot", (e) => cb(e.payload));

export const systemColorScheme = () => invoke<ColorScheme>("system_color_scheme");

/** Fires when the Linux desktop's dark-style setting changes. */
export const onColorScheme = (cb: (s: ColorScheme) => void): Promise<UnlistenFn> =>
  listen<ColorScheme>("color-scheme", (e) => cb(e.payload));
