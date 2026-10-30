import { useTheme, type ThemeChoice } from "@/app/theme";
import { usePrefs } from "@/app/prefs";
import { schemeChrome, schemeIsDark, type ChromeStyle } from "@/app/chrome";
import { schemeById } from "@/features/terminal/themes";

const OPTIONS: { value: ThemeChoice; label: string; hint: string; preview: "light" | "dark" | "auto" }[] = [
  { value: "light", label: "日间", hint: "浅色界面", preview: "light" },
  { value: "dark", label: "夜间", hint: "深色界面", preview: "dark" },
  { value: "system", label: "跟随系统", hint: "随 macOS 外观自动切换", preview: "auto" },
];

/** Swatch colours: surface, accent bar, muted bar. */
type Swatch = { bg: string; accent: string; muted: string };

/** Theme selection, with a preview swatch instead of a bare dropdown. */
export function AppearanceSection() {
  const choice = useTheme((s) => s.choice);
  const resolved = useTheme((s) => s.resolved);
  const setChoice = useTheme((s) => s.setChoice);
  const chromeStyle = usePrefs((s) => s.chromeStyle);
  const schemeId = usePrefs((s) => s.terminalScheme);
  const setPref = usePrefs((s) => s.set);

  const scheme = schemeById(schemeId);
  const fromScheme = schemeChrome(scheme);
  const dark = resolved === "dark";
  const styles: { value: ChromeStyle; label: string; hint: string; swatch: Swatch }[] = [
    {
      value: "loki",
      label: "loki",
      hint: "墨绿底色，翡翠绿点缀",
      swatch: dark
        ? { bg: "#0f1413", accent: "#2ee6a8", muted: "#5f716c" }
        : { bg: "#ffffff", accent: "#0f9d6e", muted: "#8d9390" },
    },
    {
      value: "warm",
      label: "暖灰",
      hint: "中性暖灰，保留绿色点缀",
      swatch: dark
        ? { bg: "#232221", accent: "#2ee6a8", muted: "#85827e" }
        : { bg: "#fcfaf7", accent: "#0f9d6e", muted: "#8f8b86" },
    },
    {
      value: "scheme",
      label: "跟随终端配色",
      hint: `按「${scheme.name}」计算`,
      swatch: { bg: fromScheme["--bg-elevated"], accent: fromScheme["--accent"], muted: fromScheme["--text-faint"] },
    },
  ];
  const locked = chromeStyle === "scheme";

  return (
    <section className="settings__group">
      <div className="settings__group-title">外观</div>

      <div className="settings__row">
        <div className="settings__row-main">
          <div className="settings__label">主题</div>
          <div className="settings__hint">
            {locked
              ? `界面配色跟随终端配色时，日间/夜间由配色决定（「${scheme.name}」是${schemeIsDark(scheme) ? "深色" : "浅色"}）。`
              : "终端在两种主题下都保持深色背景：浅色终端在高对比场景下看不清。"}
          </div>
        </div>
      </div>

      <div className={`theme-picker ${locked ? "theme-picker--locked" : ""}`} aria-disabled={locked}>
        {OPTIONS.map((o) => (
          <button
            key={o.value}
            className={`theme-card ${choice === o.value ? "theme-card--on" : ""}`}
            disabled={locked}
            onClick={() => setChoice(o.value)}
          >
            <span className={`theme-card__swatch theme-card__swatch--${o.preview}`}>
              <span className="theme-card__bar" />
              <span className="theme-card__bar theme-card__bar--short" />
            </span>
            <span className="theme-card__label">{o.label}</span>
            <span className="theme-card__hint">{o.hint}</span>
          </button>
        ))}
      </div>

      <div className="settings__row settings__row--spaced">
        <div className="settings__row-main">
          <div className="settings__label">界面配色</div>
          <div className="settings__hint">
            侧栏、按钮、边框这些终端以外的颜色。「跟随终端配色」会在你换终端配色时让整个界面一起换色。
          </div>
        </div>
      </div>

      <div className="theme-picker">
        {styles.map((o) => (
          <button
            key={o.value}
            className={`theme-card ${chromeStyle === o.value ? "theme-card--on" : ""}`}
            onClick={() => setPref("chromeStyle", o.value)}
          >
            <span className="theme-card__swatch" style={{ background: o.swatch.bg }}>
              <span className="theme-card__bar" style={{ background: o.swatch.accent }} />
              <span className="theme-card__bar theme-card__bar--short" style={{ background: o.swatch.muted }} />
            </span>
            <span className="theme-card__label">{o.label}</span>
            <span className="theme-card__hint">{o.hint}</span>
          </button>
        ))}
      </div>
    </section>
  );
}
