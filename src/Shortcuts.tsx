import { useEffect, useState } from "react";
import { type Action, type Hotkeys, type Shortcut, suspendHotkeys } from "./api";
import { Kbd } from "./controls";
import { modLabels, modsOf, needsModifier, sameShortcut, shortcutKeys, vkFromEvent } from "./keys";

const GROUPS: { title: string; actions: { action: Action; label: string; hint: string }[] }[] = [
  {
    title: "Arrange",
    actions: [
      { action: "tidyWindow", label: "Tidy window", hint: "Fit the active window to the layout it's closest to" },
      { action: "tidyMonitor", label: "Tidy this screen", hint: "Every window on the active window's monitor" },
      { action: "tidyAll", label: "Tidy all screens", hint: "Every window, on every monitor" },
    ],
  },
  {
    title: "Snap",
    actions: [
      { action: "snapLeft", label: "Left", hint: "Press again for two thirds, then one third" },
      { action: "snapRight", label: "Right", hint: "Press again for two thirds, then one third" },
      { action: "snapFull", label: "Fill screen", hint: "Full screen, keeping the gaps" },
      { action: "snapCenter", label: "Center", hint: "A comfortable centered window" },
    ],
  },
  {
    title: "Corners",
    actions: [{ action: "roundAll", label: "Refresh corners", hint: "Re-apply corners to every open window" }],
  },
];

export function Shortcuts({
  hotkeys,
  conflicts,
  onChange,
}: {
  hotkeys: Hotkeys;
  conflicts: Action[];
  onChange: (next: Hotkeys) => void;
}) {
  const [recording, setRecording] = useState<Action | null>(null);
  const [held, setHeld] = useState(0);
  const [nudge, setNudge] = useState(false);

  useEffect(() => {
    if (!recording) return;
    setHeld(0);
    setNudge(false);
    // Free the shortcuts so pressing an existing one records it instead of
    // triggering it.
    suspendHotkeys(true).catch(() => {});

    const assign = (shortcut: Shortcut | null) => {
      const next = { ...hotkeys };
      // One shortcut, one action: take it away from whatever had it.
      if (shortcut) {
        for (const a of Object.keys(next) as Action[]) if (sameShortcut(next[a], shortcut)) next[a] = null;
      }
      next[recording] = shortcut;
      onChange(next);
      setRecording(null);
    };

    const down = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const mods = modsOf(e);
      if (e.code === "Escape") return setRecording(null);
      if ((e.code === "Backspace" || e.code === "Delete") && mods === 0) return assign(null);
      const vk = vkFromEvent(e);
      if (vk === undefined) return setHeld(mods);
      if (mods === 0 && needsModifier(vk)) return setNudge(true);
      assign({ mods, vk });
    };
    const up = (e: KeyboardEvent) => setHeld(modsOf(e));
    const cancel = () => setRecording(null);

    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("blur", cancel);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
      window.removeEventListener("blur", cancel);
      suspendHotkeys(false).catch(() => {});
    };
  }, [recording, hotkeys, onChange]);

  return (
    <>
      {GROUPS.map((group) => (
        <section className="section" key={group.title}>
          <header className="section-head">
            <h2>{group.title}</h2>
          </header>
          <div className="card">
            {group.actions.map(({ action, label, hint }) => {
              const shortcut = hotkeys[action];
              const isRecording = recording === action;
              const taken = conflicts.includes(action) && !!shortcut;
              return (
                <div className="row" key={action}>
                  <div className="row-text">
                    <div className="row-label">{label}</div>
                    <div className="row-hint">{hint}</div>
                    {taken && <div className="row-note">Takes over from Windows or another app</div>}
                  </div>
                  <div className="row-control">
                    <button
                      type="button"
                      className={`shortcut${isRecording ? " is-recording" : ""}`}
                      onClick={() => setRecording(isRecording ? null : action)}
                      aria-label={`Change shortcut for ${label}`}
                    >
                      {isRecording ? (
                        held ? (
                          <Kbd keys={[...modLabels(held), "…"]} />
                        ) : (
                          <span className="shortcut-prompt">{nudge ? "Add Win, Ctrl or Alt" : "Press keys…"}</span>
                        )
                      ) : shortcut ? (
                        <Kbd keys={shortcutKeys(shortcut)} />
                      ) : (
                        <span className="shortcut-empty">Not set</span>
                      )}
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      ))}
      <p className="footnote">
        Click a shortcut to change it. <kbd>Backspace</kbd> clears it, <kbd>Esc</kbd> cancels.
      </p>
    </>
  );
}
