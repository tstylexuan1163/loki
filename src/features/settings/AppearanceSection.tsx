import { useTheme, type ThemeChoice } from "@/app/theme";

const OPTIONS: { value: ThemeChoice; label: string; hint: string; preview: "light" | "dark" | "auto" }[] = [
  { value: "light", label: "日间", hint: "浅色界面", preview: "light" },
  { value: "dark", label: "夜间", hint: "深色界面", preview: "dark" },
  { value: "system", label: "跟随系统", hint: "随 macOS 外观自动切换", preview: "auto" },
];

/** Theme selection, with a preview swatch instead of a bare dropdown. */
export function AppearanceSection() {
  const choice = useTheme((s) => s.choice);
  const setChoice = useTheme((s) => s.setChoice);

  return (
    <section className="settings__group">
      <div className="settings__group-title">外观</div>

      <div className="settings__row">
        <div className="settings__row-main">
          <div className="settings__label">主题</div>
          <div className="settings__hint">
            终端在两种主题下都保持深色背景：浅色终端在高对比场景下看不清。
          </div>
        </div>
      </div>

      <div className="theme-picker">
        {OPTIONS.map((o) => (
          <button
            key={o.value}
            className={`theme-card ${choice === o.value ? "theme-card--on" : ""}`}
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
    </section>
  );
}
