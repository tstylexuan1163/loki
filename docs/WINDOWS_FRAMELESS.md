# Windows 标题栏合并（无边框窗口）

> 修复「Windows 上出现两条标题栏」。给后续接手的人/模型看的说明。

---

## 1. 现象

Windows 上从上到下有两层：

1. **系统原生标题栏**（图标 + 标题 + 最小化/最大化/关闭）
2. **应用自己的标题栏**（`.titlebar`，左侧 logo + "loki"，右侧状态文字）

macOS 没有这个问题，因为窗口用了 `titleBarStyle: "Overlay"` + `hiddenTitle: true`，系统只画悬浮的红黄绿三个灯，其余交给应用自己画。

## 2. 根因

`titleBarStyle` 和 `hiddenTitle` **是 macOS 专有字段**，Windows 上被忽略，于是窗口保留完整原生装饰 —— 两条栏并存。

## 3. 改法

### 3.1 平台专属配置（新增文件）

`src-tauri/tauri.windows.conf.json`：

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "app": {
    "windows": [
      {
        "title": "loki",
        "width": 1200, "height": 760,
        "minWidth": 880, "minHeight": 560,
        "resizable": true, "fullscreen": false, "center": true,
        "dragDropEnabled": true,
        "label": "main",
        "decorations": false,
        "shadow": true
      }
    ]
  }
}
```

要点：

- Tauri 会按构建目标自动挑选 `tauri.windows.conf.json` / `tauri.macos.conf.json` / `tauri.linux.conf.json`，**macOS 的行为完全不变**。
- **`app.windows` 是数组，平台配置里的数组会整体替换而不是逐项合并**，所以这里必须把窗口对象写全（宽高、最小尺寸、label 等都要重复一遍），否则会丢字段。
- `decorations: false` 去掉原生框；`shadow: true` 保留投影，避免窗口"贴"在桌面上没有层次。
- 注意不要在这里写 `titleBarStyle` / `hiddenTitle` / `trafficLightPosition`，它们对 Windows 无意义。

### 3.2 自绘窗口按钮（新增组件）

`src/app/WindowControls.tsx`：三个按钮，用 `@tauri-apps/api/window` 的 `getCurrentWindow()`：

| 按钮 | 调用 |
|---|---|
| 最小化 | `win.minimize()` |
| 最大化 / 还原 | `win.toggleMaximize()`（图标随 `isMaximized()` 切换，监听 `onResized`） |
| 关闭 | `win.close()` |

图标是内联 SVG（10×10 描边），刻意做成 Windows 的细线风格；关闭按钮 hover 变红（`#c42b1c`），和系统一致，避免误点。

### 3.3 挂载条件

`src/app/App.tsx` 的 `TitleBar` 里：

```tsx
{/* macOS keeps the native traffic lights; elsewhere the frame is gone so
    the window needs its own controls. */}
{!isMac && <WindowControls />}
```

`isWindows` 来自 `src/app/platform.ts`（读 `navigator.userAgent`）。

**为什么用 `isWindows` 而不是 `!isMac`**：Linux 目前没有 `tauri.linux.conf.json`，仍有系统原生框；若按"非 mac 就画按钮"，Linux 上会同时出现原生按钮和自绘按钮。等将来给 Linux 加无边框配置时，再把条件放宽。

### 3.4 样式

`src/styles/app.css` 末尾新增：

- `:root[data-platform="windows"] .titlebar { padding-right: 142px; }` —— 给三个 46px 的按钮让位，避免状态文字被压住。`applyPlatform` 会把 `data-platform` 设为 `mac` / `windows` / `other`。
- `.wctl` 绝对定位贴右、`.wctl__btn` 46×40 占满标题栏高度。
- 按钮显式 `-webkit-app-region: no-drag`，不会把点击吞给拖拽区。

### 3.5 拖拽与缩放

- 拖拽沿用既有的 `data-tauri-drag-region`（WebView2 不认 CSS 的 `-webkit-app-region`，靠 Tauri 的 JS 处理）。
- **双击标题栏最大化**由 drag region 原生支持。
- 无边框窗口的边缘缩放由 Tauri 在 Windows 上自己做命中测试，不需要额外代码；`resizable: true` 必须保留。

### 3.6 窗口操作的权限（容易漏）

Tauri 2 的窗口 API 受能力（capability）系统限制。`core:window:default` **只包含只读查询**（`is-maximized`、`title` 等），
不含 `minimize` / `toggle-maximize` / `close`。不显式声明的话，按钮点下去会因权限被拒而静默失败。

`src-tauri/capabilities/default.json` 已补：

```json
"core:window:allow-minimize",
"core:window:allow-toggle-maximize",
"core:window:allow-close"
```

（双击标题栏最大化走的是 `allow-internal-toggle-maximize`，已在 default 集合里，无需额外声明。）

## 4. 验证状态

| 项 | 结果 |
|---|---|
| `cargo test` | 74 通过 |
| `cargo clippy -D warnings` | 0 警告 |
| `npx tsc --noEmit` | 通过 |
| macOS 是否受影响 | 未改 macOS 配置；`WindowControls` 在 mac 上不渲染，CSS 用 `:not([data-platform="mac"])` 限定 |
| **Windows 实际效果** | **未验证** —— 当前开发机是 macOS，无法产出/运行 Windows 包。需要在 Windows 上 `pnpm tauri build` 后目视确认 |

## 5. 需要在 Windows 上确认的点

1. 只剩一条标题栏，右侧三个按钮能最小化 / 最大化 / 还原 / 关闭。
2. 拖动标题栏能移动窗口；双击能最大化；再双击还原。
3. 从窗口四边/四角能拉伸改变大小。
4. 最大化时不会盖住任务栏。
5. 高 DPI（125% / 150%）下按钮图标不模糊、`padding-right: 142px` 仍然够用。
6. 缩到很窄时（`minWidth: 880`）标题文字不挤压按钮。

## 6. 已知取舍 / 后续可做

- **Windows 11 贴靠布局（Snap Layouts）**：鼠标悬停最大化按钮弹出布局选择，是原生框才有的能力。无边框后没有。要恢复需要自己实现 `WM_NCHITTEST` 的 `HTMAXBUTTON` 处理（Tauri 暂无现成开关），或接受没有。
- **系统圆角 / 半透明材质**：Windows 11 对无边框窗口不保证圆角；如需统一观感可加 `window-vibrancy` 或自绘圆角 + 透明背景，代价是缩放和阴影要自己处理。
- **Linux**：同一套 `decorations: false` 也适用，若将来出 Linux 版建议新增 `tauri.linux.conf.json`，并注意各桌面环境对无边框窗口的处理差异。
- **macOS 全屏**：`titleBarStyle: Overlay` 下进入全屏时 traffic lights 会隐藏，标题栏仍保留 86px 左内边距，属既有行为，本次未改。
