// Fake backend for working on the UI in a plain browser (`npm run dev`).
// Only loaded in dev builds when the Tauri runtime is missing.
//   ?conflict  pretend another app owns the "Tidy all" shortcut
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { type Config, DEFAULT_HOTKEYS, type Snapshot } from "./api";

const params = new URLSearchParams(location.search);

let config: Config = {
  enabled: true,
  roundCorners: true,
  cornerStyle: "round",
  border: "system",
  borderColor: "#5b6cf0",
  margin: 15,
  gap: 10,
  autoTidy: false,
  padMaximized: false,
  clipRadius: 12,
  ignoredApps: [],
  clipApps: ["chrome.exe"],
  hotkeys: { ...DEFAULT_HOTKEYS },
};
let startWithWindows = true;

const apps = [
  { exe: "chrome.exe", title: "New Tab - Google Chrome" },
  { exe: "code.exe", title: "rounded-corners - Visual Studio Code" },
  { exe: "discord.exe", title: "Discord" },
  { exe: "explorer.exe", title: "File Explorer" },
  { exe: "spotify.exe", title: "Spotify Premium" },
  { exe: "windowsterminal.exe", title: "Windows PowerShell" },
];

mockWindows("settings");
mockIPC(async (cmd, args) => {
  const a = args as Record<string, unknown>;
  switch (cmd) {
    case "get_state":
      return {
        config: structuredClone(config),
        conflicts: params.has("conflict") ? ["tidyAll"] : [],
        startWithWindows,
        version: "0.1.0",
      } satisfies Snapshot;
    case "set_config":
      config = structuredClone(a.config as Config);
      return structuredClone(config);
    case "set_start_with_windows":
      startWithWindows = a.enabled as boolean;
      return startWithWindows;
    case "running_apps":
      return apps;
    default:
      return null;
  }
});
