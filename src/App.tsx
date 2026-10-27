import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  type Action,
  type Config,
  DEFAULT_HOTKEYS,
  type Hotkeys,
  MAX_RADIUS,
  MAX_SPACING,
  getState,
  previewSpacing,
  quit,
  runAction,
  setConfig,
  setStartWithWindows,
} from "./api";
import { AppList } from "./AppList";
import { Row, Section, Segmented, Slider, Toggle } from "./controls";
import { Preview } from "./Preview";
import { Shortcuts } from "./Shortcuts";
import logoImg from "./assets/logo.png";
import "./App.css";

type Tab = "appearance" | "behavior" | "shortcuts";

const TABS: { value: Tab; label: string }[] = [
  { value: "appearance", label: "Appearance" },
  { value: "behavior", label: "Behavior" },
  { value: "shortcuts", label: "Shortcuts" },
];

/** While a gap slider is dragged, its value reaches the engine at most once a
 * frame so windows follow it. The engine paces each app to what it can keep
 * up with. */
const LIVE_MS = 16;
/** Sliders fire on every pixel; apply and save the full config once they
 * settle. */
const SAVE_DELAY_MS = 150;

export default function App() {
  const [config, setLocal] = useState<Config | null>(null);
  const [conflicts, setConflicts] = useState<Action[]>([]);
  const [startup, setStartup] = useState(false);
  const [version, setVersion] = useState("");
  const [tab, setTab] = useState<Tab>("appearance");
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  // The latest config, readable from timers without waiting for a render.
  const latest = useRef<Config | null>(null);
  const liveTimer = useRef<number>(undefined);
  const lastLive = useRef(0);
  const saveTimer = useRef<number>(undefined);

  const show = useCallback((next: Config) => {
    latest.current = next;
    setLocal(next);
  }, []);

  useEffect(() => {
    getState().then((s) => {
      show(s.config);
      setConflicts(s.conflicts);
      setStartup(s.startWithWindows);
      setVersion(s.version);
      // Show the window only once it has something to show.
      requestAnimationFrame(() => void getCurrentWindow().show().catch(() => {}));
    }, (e) => setError(String(e)));

    const unlisten = [
      listen<Config>("config", (e) => show(e.payload)),
      listen<Action[]>("conflicts", (e) => setConflicts(e.payload)),
    ];
    return () => unlisten.forEach((p) => p.then((off) => off()));
  }, [show]);

  useEffect(() => {
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") void getCurrentWindow().close();
    };
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, []);

  useEffect(() => {
    if (!toast) return;
    const id = setTimeout(() => setToast(null), 2200);
    return () => clearTimeout(id);
  }, [toast]);

  const save = useCallback(() => {
    if (!latest.current) return;
    setConfig(latest.current).then(
      () => setError(null),
      (e) => setError(String(e)),
    );
  }, []);

  // Just the two numbers, not the whole config: this runs every frame.
  const sendSpacing = useCallback(() => {
    liveTimer.current = undefined;
    lastLive.current = performance.now();
    if (latest.current) previewSpacing(latest.current.margin, latest.current.gap);
  }, []);

  const update = useCallback(
    (patch: Partial<Config>) => {
      if (!latest.current) return;
      show({ ...latest.current, ...patch });
      // Gap changes move the desktop with the slider as it's dragged...
      if ("margin" in patch || "gap" in patch) {
        const wait = LIVE_MS - (performance.now() - lastLive.current);
        if (wait <= 0) sendSpacing();
        else if (liveTimer.current === undefined) liveTimer.current = window.setTimeout(sendSpacing, wait);
      }
      // ...and everything is applied and saved once things settle.
      clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(save, SAVE_DELAY_MS);
    },
    [show, save, sendSpacing],
  );

  const setHotkeys = useCallback((hotkeys: Hotkeys) => update({ hotkeys }), [update]);

  const run = (action: Action, message: string) => {
    runAction(action).then(() => setToast(message), (e) => setError(String(e)));
  };

  if (!config) {
    return (
      <div className="app">
        <TitleBar />
        {error && <div className="banner">{error}</div>}
      </div>
    );
  }

  const on = config.enabled;
  const corners = on && config.roundCorners;

  return (
    <div className="app">
      <TitleBar>
        <label className="power">
          <span>{on ? "On" : "Paused"}</span>
          <Toggle checked={on} onChange={(enabled) => update({ enabled })} label="Fillet is on" />
        </label>
      </TitleBar>

      <nav className="tabs">
        <Segmented value={tab} options={TABS} onChange={setTab} label="Settings pages" />
      </nav>

      <main className="content" key={tab}>
        {error && <div className="banner">{error}</div>}

        {tab === "appearance" && (
          <>
            <Preview config={config} />

            <Section title="Corners">
              <Row label="Round window corners" hint="Applied to every window as it opens" disabled={!on}>
                <Toggle
                  checked={config.roundCorners}
                  onChange={(roundCorners) => update({ roundCorners })}
                  label="Round window corners"
                  disabled={!on}
                />
              </Row>
              <Row label="Radius" disabled={!corners}>
                <Segmented
                  value={config.cornerStyle}
                  options={[
                    { value: "round", label: "Rounded" },
                    { value: "small", label: "Subtle" },
                  ]}
                  onChange={(cornerStyle) => update({ cornerStyle })}
                  label="Corner radius"
                  disabled={!corners}
                />
              </Row>
              <Row label="Border" disabled={!on}>
                {config.border === "custom" && (
                  <label className="swatch" style={{ background: config.borderColor }}>
                    <input
                      type="color"
                      value={config.borderColor}
                      aria-label="Border color"
                      disabled={!on}
                      onChange={(e) => update({ borderColor: e.target.value })}
                    />
                  </label>
                )}
                <Segmented
                  value={config.border}
                  options={[
                    { value: "system", label: "System" },
                    { value: "none", label: "None" },
                    { value: "custom", label: "Color" },
                  ]}
                  onChange={(border) => update({ border })}
                  label="Window border"
                  disabled={!on}
                />
              </Row>
              <Row label="Highlight the active window" hint="A soft accent border on the window you're using" disabled={!on}>
                <Toggle
                  checked={config.highlightActive}
                  onChange={(highlightActive) => update({ highlightActive })}
                  label="Highlight the active window"
                  disabled={!on}
                />
              </Row>
            </Section>

            <Section
              title="Gaps"
              action={
                <button type="button" className="link" disabled={!on} onClick={() => run("tidyAll", "Windows tidied")}>
                  Tidy all windows
                </button>
              }
            >
              <Row label="Screen edge" hint="Space around the edge of each screen" disabled={!on}>
                <Slider
                  value={config.margin}
                  min={0}
                  max={MAX_SPACING}
                  onChange={(margin) => update({ margin })}
                  label="Screen edge gap"
                  disabled={!on}
                />
              </Row>
              <Row label="Between windows" hint="Space between windows side by side" disabled={!on}>
                <Slider
                  value={config.gap}
                  min={0}
                  max={MAX_SPACING}
                  onChange={(gap) => update({ gap })}
                  label="Gap between windows"
                  disabled={!on}
                />
              </Row>
            </Section>
          </>
        )}

        {tab === "behavior" && (
          <>
            <Section title="Automatic">
              <Row
                label="Tidy snapped windows"
                hint="Add the gaps when you snap a window to an edge or a snap layout"
                disabled={!on}
              >
                <Toggle
                  checked={config.autoTidy}
                  onChange={(autoTidy) => update({ autoTidy })}
                  label="Tidy snapped windows"
                  disabled={!on}
                />
              </Row>
              <Row
                label="Keep maximized windows round"
                hint="Maximize fills the screen with gaps instead of edge to edge"
                disabled={!on}
              >
                <Toggle
                  checked={config.padMaximized}
                  onChange={(padMaximized) => update({ padMaximized })}
                  label="Keep maximized windows round"
                  disabled={!on}
                />
              </Row>
            </Section>

            <Section title="Apps">
              <div className="stack">
                <div className="stack-text">
                  <div className="row-label">Leave alone</div>
                  <div className="row-hint">Fillet never rounds or moves these apps</div>
                </div>
                <AppList
                  apps={config.ignoredApps}
                  onChange={(ignoredApps) => update({ ignoredApps })}
                  empty="No apps"
                  label="Add an app to leave alone"
                />
              </div>
              <div className="stack">
                <div className="stack-text">
                  <div className="row-label">Force round corners</div>
                  <div className="row-hint">For apps that ignore Windows' corner setting. Fillet trims them instead.</div>
                </div>
                <AppList
                  apps={config.clipApps}
                  onChange={(clipApps) => update({ clipApps })}
                  empty="No apps"
                  label="Add an app to force round"
                />
              </div>
              <Row label="Trimmed radius" disabled={!corners || config.clipApps.length === 0}>
                <Slider
                  value={config.clipRadius}
                  min={2}
                  max={MAX_RADIUS}
                  onChange={(clipRadius) => update({ clipRadius })}
                  label="Trimmed corner radius"
                  disabled={!corners || config.clipApps.length === 0}
                />
              </Row>
            </Section>

            <Section title="General">
              <Row label="Start with Windows" hint="Fillet runs quietly in the tray">
                <Toggle
                  checked={startup}
                  onChange={(enabled) =>
                    setStartWithWindows(enabled).then(setStartup, (e) => setError(String(e)))
                  }
                  label="Start with Windows"
                />
              </Row>
              <Row label={`Fillet ${version}`} hint="Closing this window keeps Fillet running in the tray">
                <button type="button" className="button" onClick={() => void quit()}>
                  Quit Fillet
                </button>
              </Row>
            </Section>
          </>
        )}

        {tab === "shortcuts" && (
          <>
            <Shortcuts hotkeys={config.hotkeys} conflicts={conflicts} onChange={setHotkeys} />
            <div className="center">
              <button type="button" className="link" onClick={() => setHotkeys({ ...DEFAULT_HOTKEYS })}>
                Reset to defaults
              </button>
            </div>
          </>
        )}
      </main>

      <div className={`toast${toast ? " is-shown" : ""}`} role="status">
        {toast}
      </div>
    </div>
  );
}

function TitleBar({ children }: { children?: React.ReactNode }) {
  const win = getCurrentWindow();
  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="brand" data-tauri-drag-region>
        <img src={logoImg} alt="" width={20} height={20} />
        <span data-tauri-drag-region>Fillet</span>
      </div>
      <div className="titlebar-end">
        {children}
        <div className="winbtns">
          <button type="button" aria-label="Minimize" onClick={() => void win.minimize()}>
            <svg viewBox="0 0 10 10" width="10" height="10">
              <path d="M1 5h8" stroke="currentColor" strokeWidth="1" />
            </svg>
          </button>
          <button type="button" aria-label="Close" className="is-close" onClick={() => void win.close()}>
            <svg viewBox="0 0 10 10" width="10" height="10">
              <path d="M1 1l8 8M9 1l-8 8" stroke="currentColor" strokeWidth="1" />
            </svg>
          </button>
        </div>
      </div>
    </header>
  );
}
