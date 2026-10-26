import { useEffect, useMemo, useRef, useState } from "react";
import { type RunningApp, runningApps } from "./api";

/** Normalise what someone typed into an executable name: "Code" → "code.exe". */
function toExe(text: string) {
  const name = text.trim().toLowerCase();
  if (!name) return "";
  return name.includes(".") ? name : `${name}.exe`;
}

export function AppList({
  apps,
  onChange,
  empty,
  label,
}: {
  apps: string[];
  onChange: (next: string[]) => void;
  empty: string;
  label: string;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [running, setRunning] = useState<RunningApp[]>([]);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    runningApps().then(setRunning, () => setRunning([]));
    const outside = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", esc, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", esc, true);
    };
  }, [open]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    return running.filter(
      (a) => !apps.includes(a.exe) && (!q || a.exe.includes(q) || a.title.toLowerCase().includes(q)),
    );
  }, [running, apps, query]);

  const add = (exe: string) => {
    if (exe && !apps.includes(exe)) onChange([...apps, exe].sort());
    setOpen(false);
  };

  return (
    <div className="chips">
      {apps.length === 0 && <span className="chips-empty">{empty}</span>}
      {apps.map((exe) => (
        <span className="chip" key={exe}>
          {exe}
          <button type="button" aria-label={`Remove ${exe}`} onClick={() => onChange(apps.filter((a) => a !== exe))}>
            <svg viewBox="0 0 10 10" width="8" height="8">
              <path d="M1 1l8 8M9 1l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
            </svg>
          </button>
        </span>
      ))}
      <div className="picker" ref={box}>
        <button type="button" className="chip chip-add" onClick={() => setOpen(!open)} aria-expanded={open}>
          <svg viewBox="0 0 10 10" width="9" height="9">
            <path d="M5 1v8M1 5h8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
          Add app
        </button>
        {open && (
          <div className="popover" role="dialog" aria-label={label}>
            <input
              autoFocus
              value={query}
              placeholder="Search open apps or type a name"
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") add(matches[0] ? matches[0].exe : toExe(query));
              }}
            />
            <div className="popover-list">
              {matches.map((a) => (
                <button type="button" key={a.exe} onClick={() => add(a.exe)}>
                  <span className="popover-exe">{a.exe}</span>
                  <span className="popover-title">{a.title}</span>
                </button>
              ))}
              {matches.length === 0 && query.trim() && (
                <button type="button" onClick={() => add(toExe(query))}>
                  <span className="popover-exe">Add “{toExe(query)}”</span>
                </button>
              )}
              {matches.length === 0 && !query.trim() && <div className="popover-empty">No other apps open</div>}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
