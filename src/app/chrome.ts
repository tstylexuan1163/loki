/**
 * The colours of the app around the terminal ("chrome").
 *
 * `loki` is the stylesheet's own palette. `warm` swaps its green-tinted greys
 * for warm neutral ones and keeps the emerald accent. `scheme` derives the
 * whole chrome from the terminal colour scheme, so switching to Dracula or
 * Solarized re-dresses the sidebar, buttons and borders to match instead of
 * leaving a green app around a purple terminal. The idea, and the mixing
 * ratios, follow ELYTerminal's chrome palette.
 *
 * Everything lands as inline custom properties on <html>, which beat the
 * stylesheet's `:root` rules; `loki` simply removes them again.
 */
import { schemeById, type TerminalScheme } from "@/features/terminal/themes";
import type { ResolvedTheme } from "@/app/theme";

export type ChromeStyle = "loki" | "warm" | "scheme";

type Vars = Record<string, string>;

const WARM: Record<ResolvedTheme, Vars> = {
  dark: {
    "--bg": "#1c1b1a",
    "--bg-elevated": "#232221",
    "--bg-raised": "#2a2928",
    "--bg-hover": "#323130",
    "--bg-sunken": "#171615",
    "--border": "#34322f",
    "--border-strong": "#4a4744",
    "--text": "#f3f1f0",
    "--text-dim": "#b9b6b2",
    "--text-faint": "#85827e",
    "--scrim": "rgba(10, 9, 8, 0.6)",
  },
  light: {
    "--bg": "#f7f5f1",
    "--bg-elevated": "#fcfaf7",
    "--bg-raised": "#f1efeb",
    "--bg-hover": "#ebe8e4",
    "--bg-sunken": "#ffffff",
    "--border": "#e1dfdb",
    "--border-strong": "#cbc8c3",
    "--text": "#181613",
    "--text-dim": "#5f5b57",
    "--text-faint": "#8f8b86",
    "--scrim": "rgba(24, 22, 19, 0.3)",
  },
};

type Rgb = [number, number, number];

/** `#rgb`, `#rrggbb` or `rgb(a)(…)` to 0–255 channels; anything else is black. */
function parse(color: string): Rgb {
  const c = color.trim();
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(c)?.[1];
  if (hex) {
    const full = hex.length === 3 ? [...hex].map((d) => d + d).join("") : hex;
    return [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16)) as Rgb;
  }
  const nums = /^rgba?\(([^)]+)\)$/i.exec(c)?.[1].split(",").map((n) => parseFloat(n));
  return nums && nums.length >= 3 ? [nums[0], nums[1], nums[2]] : [0, 0, 0];
}

const hex = ([r, g, b]: Rgb) =>
  "#" + [r, g, b].map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, "0")).join("");

/** `share` of the way from `a` to `b`. */
const mix = (a: Rgb, b: Rgb, share: number): Rgb =>
  [0, 1, 2].map((i) => a[i] + (b[i] - a[i]) * share) as Rgb;

const alpha = ([r, g, b]: Rgb, a: number) => `rgba(${Math.round(r)}, ${Math.round(g)}, ${Math.round(b)}, ${a})`;

/** Relative luminance, 0 (black) to 1 (white). */
function luminance([r, g, b]: Rgb): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** Whether a scheme reads as dark, which decides the app's day/night mode. */
export function schemeIsDark(s: TerminalScheme): boolean {
  return luminance(parse(s.theme.background)) < 0.2;
}

/** The chrome for one terminal scheme. Exported for tests and previews. */
export function schemeChrome(s: TerminalScheme): Vars {
  const bg = parse(s.theme.background);
  const fg = parse(s.theme.foreground);
  const dark = schemeIsDark(s);
  const tint = (share: number) => hex(mix(bg, fg, share));
  // The scheme's own green stands in for loki's emerald: every scheme has
  // one, and it already reads as "connected / go" in that scheme.
  const accent = parse(s.theme.green);
  const contrast = luminance(accent) > 0.35 ? (dark ? bg : fg) : [255, 255, 255];
  const red = parse(s.theme.red);
  return {
    "--bg": tint(0),
    "--bg-elevated": tint(0.04),
    "--bg-raised": tint(0.07),
    "--bg-hover": tint(0.1),
    "--bg-sunken": hex(mix(bg, dark ? [0, 0, 0] : [255, 255, 255], dark ? 0.25 : 0.6)),
    "--border": tint(0.14),
    "--border-strong": tint(0.26),
    "--text": hex(fg),
    "--text-dim": tint(0.7),
    "--text-faint": tint(0.5),
    "--accent": hex(accent),
    "--accent-dim": hex(mix(accent, bg, 0.3)),
    "--accent-soft": alpha(accent, dark ? 0.12 : 0.1),
    "--accent-glow": alpha(accent, dark ? 0.22 : 0.18),
    "--accent-contrast": hex(contrast as Rgb),
    "--gold": s.theme.yellow,
    "--warn": s.theme.yellow,
    "--danger": hex(red),
    "--danger-soft": alpha(red, 0.12),
    "--scrim": alpha(dark ? mix(bg, [0, 0, 0], 0.5) : mix(bg, fg, 0.6), dark ? 0.6 : 0.3),
  };
}

/** Every property this module may set, so switching styles clears the old ones. */
const OWNED = new Set([...Object.keys(WARM.dark), ...Object.keys(schemeChrome(schemeById("loki")))]);

/**
 * Puts the chrome on the document. `theme` is the day/night choice the user
 * made; in `scheme` style the scheme decides instead, and `data-theme` is set
 * to match so the stylesheet's light-only rules follow.
 */
export function applyChrome(style: ChromeStyle, schemeId: string, theme: ResolvedTheme) {
  const root = document.documentElement;
  for (const name of OWNED) root.style.removeProperty(name);
  let vars: Vars = {};
  let mode = theme;
  if (style === "warm") vars = WARM[theme];
  if (style === "scheme") {
    const s = schemeById(schemeId);
    vars = schemeChrome(s);
    mode = schemeIsDark(s) ? "dark" : "light";
  }
  root.dataset.theme = mode;
  for (const [name, value] of Object.entries(vars)) root.style.setProperty(name, value);
}
