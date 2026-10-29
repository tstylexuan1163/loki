import { FONT_CHOICES, usePrefs, type CursorStyle } from "@/app/prefs";
import { Segmented, Switch } from "@/components/ui";
import { keyLabel } from "@/app/platform";
import { TERMINAL_SCHEMES, schemeById } from "@/features/terminal/themes";

/** Terminal appearance and behaviour; applied live to every open pane. */
export function TerminalSection() {
  const p = usePrefs();
  const set = p.set;
  const scheme = schemeById(p.terminalScheme).theme;

  return (
    <>
      <section className="settings__group">
        <div className="settings__group-title">配色</div>
        <div className="scheme-grid">
          {TERMINAL_SCHEMES.map((s) => (
            <button
              key={s.id}
              className={`scheme ${p.terminalScheme === s.id ? "scheme--on" : ""}`}
              onClick={() => set("terminalScheme", s.id)}
              style={{ background: s.theme.background, color: s.theme.foreground }}
              title={s.name}
            >
              <span className="scheme__prompt">
                <span style={{ color: s.theme.green }}>user@host</span>:
                <span style={{ color: s.theme.blue }}>~</span>$ ls
              </span>
              <span className="scheme__dots">
                {[s.theme.red, s.theme.green, s.theme.yellow, s.theme.blue, s.theme.magenta, s.theme.cyan].map(
                  (c, i) => (
                    <i key={i} style={{ background: c }} />
                  ),
                )}
              </span>
              <span className="scheme__name">{s.name}</span>
            </button>
          ))}
        </div>
      </section>

      <section className="settings__group">
        <div className="settings__group-title">字体</div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">字体</div>
          </div>
          <select
            className="field__input"
            style={{ width: 200 }}
            value={p.fontFamily}
            onChange={(e) => set("fontFamily", e.target.value)}
          >
            {FONT_CHOICES.map((f) => (
              <option key={f.label} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
        </div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">字号</div>
            <div className="settings__hint">{p.fontSize} px</div>
          </div>
          <input
            type="range"
            min={10}
            max={22}
            value={p.fontSize}
            onChange={(e) => set("fontSize", Number(e.target.value))}
          />
        </div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">行高</div>
            <div className="settings__hint">{p.lineHeight.toFixed(2)}</div>
          </div>
          <input
            type="range"
            min={1}
            max={1.6}
            step={0.05}
            value={p.lineHeight}
            onChange={(e) => set("lineHeight", Number(e.target.value))}
          />
        </div>
        <div
          className="term-preview"
          style={{ fontFamily: p.fontFamily, fontSize: p.fontSize, lineHeight: p.lineHeight }}
        >
          <span style={{ color: scheme.green }}>deploy@prod-api</span>:<span style={{ color: scheme.blue }}>~/app</span>$ systemctl status nginx
          <br />
          <span style={{ color: scheme.green }}>●</span> nginx.service - A high performance web server
          <br />
          {"   "}Active: <span style={{ color: scheme.green }}>active (running)</span> since Mon 09:12:44 UTC
        </div>
      </section>

      <section className="settings__group">
        <div className="settings__group-title">光标</div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">样式</div>
          </div>
          <Segmented<CursorStyle>
            value={p.cursorStyle}
            onChange={(v) => set("cursorStyle", v)}
            options={[
              { value: "block", label: "方块" },
              { value: "bar", label: "竖线" },
              { value: "underline", label: "下划线" },
            ]}
          />
        </div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">闪烁</div>
          </div>
          <Switch checked={p.cursorBlink} onChange={(v) => set("cursorBlink", v)} />
        </div>
      </section>

      <section className="settings__group">
        <div className="settings__group-title">行为</div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">选中即复制</div>
            <div className="settings__hint">在终端里选中文字后自动复制到剪贴板。{keyLabel("C")} 总是可用。</div>
          </div>
          <Switch checked={p.copyOnSelect} onChange={(v) => set("copyOnSelect", v)} />
        </div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">回滚行数</div>
            <div className="settings__hint">每个终端最多保留的历史行。</div>
          </div>
          <Segmented<number>
            value={p.scrollback}
            onChange={(v) => set("scrollback", v)}
            options={[
              { value: 5000, label: "5k" },
              { value: 10000, label: "10k" },
              { value: 50000, label: "50k" },
            ]}
          />
        </div>
        <div className="settings__row">
          <div className="settings__row-main">
            <div className="settings__label">服务器状态栏</div>
            <div className="settings__hint">在 SSH 终端下方显示 CPU、内存、磁盘，每 5 秒采样一次。</div>
          </div>
          <Switch checked={p.showStats} onChange={(v) => set("showStats", v)} />
        </div>
      </section>
    </>
  );
}
