import { invoke } from "@tauri-apps/api/core";

// Mirrors src-tauri/src/config.rs. Keep the two in sync.

export type CornerStyle = "round" | "small";
export type Border = "system" | "none" | "custom";
export type Highlight = "off" | "accent" | "custom";

/** `mods` uses RegisterHotKey flags (Alt 1, Ctrl 2, Shift 4, Win 8); `vk` is a virtual-key code. */
export interface Shortcut {
  mods: number;
  vk: number;
}

export type Action =
  | "tidyWindow"
  | "tidyMonitor"
  | "tidyAll"
  | "snapLeft"
  | "snapRight"
  | "snapFull"
  | "snapCenter"
  | "roundAll";

export type Hotkeys = Record<Action, Shortcut | null>;

export interface Config {
  enabled: boolean;
  roundCorners: boolean;
  cornerStyle: CornerStyle;
  border: Border;
  borderColor: string;
  highlight: Highlight;
  highlightColor: string;
  dimInactive: boolean;
  dimAmount: number;
  margin: number;
  gap: number;
  autoTidy: boolean;
  padMaximized: boolean;
  clipRadius: number;
  ignoredApps: string[];
  clipApps: string[];
  hotkeys: Hotkeys;
}

export interface Snapshot {
  config: Config;
  conflicts: Action[];
  startWithWindows: boolean;
  version: string;
}

export interface RunningApp {
  exe: string;
  title: string;
}

export const MAX_SPACING = 64;
export const MAX_RADIUS = 32;
export const MIN_DIM = 5;
export const MAX_DIM = 80;

export const DEFAULT_HOTKEYS: Hotkeys = {
  tidyWindow: { mods: 9, vk: 0x0d },
  tidyMonitor: { mods: 9, vk: 0x41 },
  tidyAll: { mods: 13, vk: 0x41 },
  snapLeft: { mods: 9, vk: 0x25 },
  snapRight: { mods: 9, vk: 0x27 },
  snapFull: { mods: 9, vk: 0x26 },
  snapCenter: { mods: 9, vk: 0x28 },
  roundAll: { mods: 9, vk: 0x43 },
};

export const getState = () => invoke<Snapshot>("get_state");
/** Apply and save a config. */
export const setConfig = (config: Config) => invoke<Config>("set_config", { config });
/** Move windows to new gaps while a slider is dragged, without saving. Fire
 * and forget: it runs every frame. */
export const previewSpacing = (margin: number, gap: number) =>
  void invoke("preview_spacing", { margin, gap }).catch(() => {});
/** Change the dim amount while its slider is dragged, without saving. */
export const previewDim = (amount: number) => void invoke("preview_dim", { amount }).catch(() => {});
export const setStartWithWindows = (enabled: boolean) => invoke<boolean>("set_start_with_windows", { enabled });
export const runningApps = () => invoke<RunningApp[]>("running_apps");
export const runAction = (action: Action) => invoke<void>("run_action", { action });
export const suspendHotkeys = (suspend: boolean) => invoke<void>("suspend_hotkeys", { suspend });
export const quit = () => invoke<void>("quit");
