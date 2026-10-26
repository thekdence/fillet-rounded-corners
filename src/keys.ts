import type { Shortcut } from "./api";

export const MOD_ALT = 1;
export const MOD_CTRL = 2;
export const MOD_SHIFT = 4;
export const MOD_WIN = 8;

/** KeyboardEvent.code → Windows virtual-key code. */
const CODES: Record<string, number> = {
  Enter: 0x0d,
  NumpadEnter: 0x0d,
  Space: 0x20,
  Tab: 0x09,
  PageUp: 0x21,
  PageDown: 0x22,
  End: 0x23,
  Home: 0x24,
  ArrowLeft: 0x25,
  ArrowUp: 0x26,
  ArrowRight: 0x27,
  ArrowDown: 0x28,
  PrintScreen: 0x2c,
  Insert: 0x2d,
  Delete: 0x2e,
  Pause: 0x13,
  NumpadMultiply: 0x6a,
  NumpadAdd: 0x6b,
  NumpadSubtract: 0x6d,
  NumpadDecimal: 0x6e,
  NumpadDivide: 0x6f,
  Semicolon: 0xba,
  Equal: 0xbb,
  Comma: 0xbc,
  Minus: 0xbd,
  Period: 0xbe,
  Slash: 0xbf,
  Backquote: 0xc0,
  BracketLeft: 0xdb,
  Backslash: 0xdc,
  BracketRight: 0xdd,
  Quote: 0xde,
};
for (let i = 0; i < 26; i++) CODES[`Key${String.fromCharCode(65 + i)}`] = 0x41 + i;
for (let i = 0; i < 10; i++) {
  CODES[`Digit${i}`] = 0x30 + i;
  CODES[`Numpad${i}`] = 0x60 + i;
}
for (let i = 1; i <= 24; i++) CODES[`F${i}`] = 0x6f + i;

const LABELS: Record<number, string> = {
  0x0d: "Enter",
  0x20: "Space",
  0x09: "Tab",
  0x21: "Page Up",
  0x22: "Page Down",
  0x23: "End",
  0x24: "Home",
  0x25: "←",
  0x26: "↑",
  0x27: "→",
  0x28: "↓",
  0x2c: "Print Screen",
  0x2d: "Insert",
  0x2e: "Delete",
  0x13: "Pause",
  0x6a: "Num *",
  0x6b: "Num +",
  0x6d: "Num -",
  0x6e: "Num .",
  0x6f: "Num /",
  0xba: ";",
  0xbb: "=",
  0xbc: ",",
  0xbd: "-",
  0xbe: ".",
  0xbf: "/",
  0xc0: "`",
  0xdb: "[",
  0xdc: "\\",
  0xdd: "]",
  0xde: "'",
};

/** The virtual key for a key press, or undefined for a lone modifier. */
export function vkFromEvent(e: KeyboardEvent): number | undefined {
  if (CODES[e.code] !== undefined) return CODES[e.code];
  // Some keyboards and remapping tools send no `code`; fall back to the
  // character itself.
  const key = e.key.length === 1 ? e.key.toUpperCase() : "";
  if (/^[A-Z0-9]$/.test(key)) return key.charCodeAt(0);
  return undefined;
}

export function vkLabel(vk: number): string {
  if (LABELS[vk]) return LABELS[vk];
  if (vk >= 0x41 && vk <= 0x5a) return String.fromCharCode(vk);
  if (vk >= 0x30 && vk <= 0x39) return String.fromCharCode(vk);
  if (vk >= 0x60 && vk <= 0x69) return `Num ${vk - 0x60}`;
  if (vk >= 0x70 && vk <= 0x87) return `F${vk - 0x6f}`;
  return `Key ${vk}`;
}

export function modLabels(mods: number): string[] {
  const out: string[] = [];
  if (mods & MOD_WIN) out.push("Win");
  if (mods & MOD_CTRL) out.push("Ctrl");
  if (mods & MOD_ALT) out.push("Alt");
  if (mods & MOD_SHIFT) out.push("Shift");
  return out;
}

export const shortcutKeys = (s: Shortcut) => [...modLabels(s.mods), vkLabel(s.vk)];

export function modsOf(e: KeyboardEvent): number {
  return (
    (e.metaKey ? MOD_WIN : 0) | (e.ctrlKey ? MOD_CTRL : 0) | (e.altKey ? MOD_ALT : 0) | (e.shiftKey ? MOD_SHIFT : 0)
  );
}

/** F13–F24 are free on almost every keyboard, so they're fine on their own. */
export const needsModifier = (vk: number) => !(vk >= 0x7c && vk <= 0x87);

export const sameShortcut = (a: Shortcut | null, b: Shortcut | null) =>
  !!a && !!b && a.mods === b.mods && a.vk === b.vk;
