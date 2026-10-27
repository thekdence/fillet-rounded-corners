import { useState } from "react";
import type { Config } from "./api";

// A miniature desktop: one window on the left, two stacked on the right,
// laid out with the same maths as src-tauri/src/layout.rs. The screen is a
// small virtual one so the gaps and corners read clearly at this size.
const SCREEN_W = 720;
const SCREEN_H = 372;
const TASKBAR = 22;

type Kind = "editor" | "browser" | "chat";

export function Preview({ config }: { config: Config }) {
  // Click a window to focus it, to try the highlight and dimming.
  const [focus, setFocus] = useState<Kind>("editor");
  const W = SCREEN_W;
  const H = SCREEN_H - TASKBAR;
  const { margin: m, gap: g } = config;
  const midX = Math.round(W / 2 - g / 2);
  const midY = Math.round(H / 2 - g / 2);

  const windows: { x: number; y: number; w: number; h: number; kind: Kind }[] = [
    { x: m, y: m, w: midX - m, h: H - m * 2, kind: "editor" },
    { x: midX + g, y: m, w: W - m - midX - g, h: midY - m, kind: "browser" },
    { x: midX + g, y: midY + g, w: W - m - midX - g, h: H - m - midY - g, kind: "chat" },
  ];

  const radius = config.roundCorners ? (config.cornerStyle === "round" ? 8 : 4) : 0;
  const border =
    config.border === "none"
      ? "transparent"
      : config.border === "custom"
        ? config.borderColor
        : "var(--win-border)";

  const highlight =
    config.highlight === "accent" ? "var(--accent)" : config.highlight === "custom" ? config.highlightColor : null;

  const pct = (v: number, of: number) => `${(v / of) * 100}%`;

  return (
    <div className={`preview${config.enabled ? "" : " is-paused"}`} aria-hidden>
      <div className="screen">
        {windows.map((w) => (
          <div
            key={w.kind}
            className={`pwin pwin-${w.kind}`}
            onPointerDown={() => setFocus(w.kind)}
            style={{
              left: pct(w.x, W),
              top: pct(w.y, SCREEN_H),
              width: pct(w.w, W),
              height: pct(w.h, SCREEN_H),
              borderRadius: `calc(${radius} * var(--u))`,
              borderColor: w.kind === focus && highlight ? highlight : border,
              zIndex: w.kind === focus ? 2 : undefined,
            }}
          >
            <div className="pwin-bar">
              <i />
              <i />
              <i />
            </div>
            <div className="pwin-body">
              {w.kind === "editor" && <div className="pwin-side" />}
              <div className="pwin-lines">
                <b style={{ width: "62%" }} />
                <b style={{ width: "84%" }} />
                <b style={{ width: "48%" }} />
                {w.kind === "editor" && (
                  <>
                    <b style={{ width: "72%" }} />
                    <b style={{ width: "56%" }} />
                    <b style={{ width: "80%" }} />
                  </>
                )}
              </div>
            </div>
          </div>
        ))}
        <div className="pdim" style={{ opacity: config.enabled && config.dimInactive ? config.dimAmount / 100 : 0 }} />
        <div className="ptaskbar" style={{ height: pct(TASKBAR, SCREEN_H) }}>
          <i className="is-start" />
          <i />
          <i />
          <i />
        </div>
        {!config.enabled && <div className="preview-paused">Paused</div>}
      </div>
    </div>
  );
}
