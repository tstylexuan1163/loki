# 检查更新 + 安装界面语言（v0.9.0）

> 两项改动：①应用内检查更新、提示、点击安装；②Windows 安装器跟随系统语言。
> 写给后续接手的人/模型。

---

## 一、检查更新

### 1.1 使用体验

| 入口 | 行为 |
|---|---|
| 启动后约 4 秒 | 后台静默检查一次。有新版才提示，**失败不打扰**（网络断、仓库没发布过都静默） |
| 设置 → 关于 | 「检查更新」按钮 + 状态文字（正在检查 / 已是最新 / 发现新版本 x.y.z / 下载百分比 / 错误） |
| 发现新版时 | 右下角浮出提示条（非模态，不打断终端会话），显示版本号、更新说明、「稍后」「下载并安装」 |
| 点「下载并安装」 | 进度条 + 百分比；失败显示原因并可「重试」 |

选择浮层而不是模态框：用户可能正在终端里跑命令，强制弹窗会打断操作。「稍后」只对当前版本生效（`dismissed`），下次启动会再提示。

### 1.2 实现方式

用官方 `tauri-plugin-updater`（Rust 侧），但**不用它的 npm 包**：

- 本项目只调用 Rust API，自己包了两条命令，前端走 `invoke`。
- 好处：不需要 `@tauri-apps/plugin-updater`，减少一个前端依赖。

命令（`src-tauri/src/commands.rs`）：

| 命令 | 返回 | 说明 |
|---|---|---|
| `update_check` | `Option<UpdateInfo>` | `null` = 已是最新（和"检查失败"区分开） |
| `update_install` | `()` | 下载→校验签名→交给平台安装器；进度通过 `update-progress` 事件推送 |

`UpdateInfo`：`{ currentVersion, version, notes, date }`。

`update_install` 用一个 `update_busy` 互斥位防止重复点击（两次下载会抢同一个临时文件）。macOS 安装成功后调用 `app.restart()` 重新执行新二进制；Windows 交给 NSIS 安装器自行重启。

### 1.3 配置

`src-tauri/tauri.conf.json`：

```json
"plugins": {
  "updater": {
    "endpoints": ["https://github.com/tstylexuan1163/loki/releases/latest/download/latest.json"],
    "pubkey": "<minisign 公钥>"
  }
}
```

`src-tauri/tauri.release.conf.json`（**只给 CI 用**）：

```json
{ "bundle": { "createUpdaterArtifacts": true } }
```

放在单独文件、由 CI 用 `--config` 传入，是为了让**本地 `pnpm tauri build` 不需要签名私钥也能跑**。若直接写进主配置，没有 `TAURI_SIGNING_PRIVATE_KEY` 时构建会失败，会挡住本地开发。

### 1.4 安全属性（重要，别去掉）

更新包必须用 minisign 私钥签名，客户端用配置里的公钥校验。**校验失败一律不安装**。

这一条我用伪造数据实测过：本地起一个假的 `latest.json` 指向一个假包、签名随便填，点「下载并安装」后报
`Invalid encoding in minisign data`，**没有任何文件被写入或执行**。

不要为了方便改成"跳过签名校验"——这个应用管着 SSH 私钥，装进来一个未经验证的二进制等于把机器交出去。

### 1.5 发版流程

1. 改版本号（`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`）。
2. 写 `docs/release-notes/vX.Y.Z.md`（没有就用空说明）。
3. 提交并打 tag：`git tag vX.Y.Z && git push origin vX.Y.Z`。
4. CI（`.github/workflows/release.yml`）会并行构建 macOS(arm64/x64) + Windows，产出安装包、`.sig` 与 `latest.json`，最后自动把 draft 转正。

**必需的两个仓库 secret**：

| Secret | 内容 |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | 私钥内容（或私钥文件路径） |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 私钥口令；本次生成时用的是**空口令**，可留空 |

私钥本次生成在 `~/.tauri/loki-updater.key`（**不要提交进仓库，不要贴到聊天/日志里**）。公钥已在配置中。

> 若仓库是 **private**，`releases/latest/download/latest.json` 匿名取不到，更新检查会失败。要么把仓库设为 public，要么把 `latest.json` 放到别的公开地址并改 `endpoints`。

### 1.6 改动的文件

```
src-tauri/Cargo.toml                     + tauri-plugin-updater
src-tauri/src/lib.rs                     注册插件、update_busy 状态、注册两条命令
src-tauri/src/commands.rs                update_check / update_install（含 macOS 重启）
src-tauri/tauri.conf.json                plugins.updater（endpoints + pubkey）
src-tauri/tauri.release.conf.json        新增：createUpdaterArtifacts（CI 专用）
.github/workflows/release.yml            --config 传入 + 签名密钥环境变量
src/lib/api.ts                           UpdateInfo、updateApi
src/app/updates.ts                       新增：更新状态机
src/components/UpdateBanner.tsx          新增：右下角提示条
src/app/App.tsx                          启动静默检查、监听 update-progress、挂载提示条
src/features/settings/SettingsWindow.tsx 关于页：「检查更新」+ 状态 + 数据位置改为跨平台
src/styles/app.css                       .updatebanner 样式
```

### 1.7 验证状态

| 项 | 结果 |
|---|---|
| 启动自动检查 → 右下角提示条（版本号 + 说明） | ✅ 用本地假 `latest.json`（9.9.9）实测 |
| 设置 → 关于 → 手动检查 → 显示「发现新版本」+ 安装按钮 | ✅ 同上 |
| 下载进度 / 失败重试 UI | ✅ 失败路径实测（伪造签名） |
| **签名校验拦截未签名包** | ✅ 实测：`Invalid encoding in minisign data`，未安装 |
| **真实发布包的完整升级**（下载→校验通过→安装→重启） | ✅ **已验证**（见 §1.8） |
| Windows 上 NSIS 静默安装 + 自动重启 | ❌ 未验证（无 Windows 环境） |

### 1.8 真实升级实测记录

v0.9.0 发布后做了一次端到端验证：把本机构建的版本号临时改成 **0.8.9**，启动后让它去升真正的 v0.9.0 发布包。

| 步骤 | 观察到的结果 |
|---|---|
| 启动后自动检查 | 右下角浮出「发现新版本 0.9.0」——命中的是真实 GitHub Release |
| 点「下载并安装」 | 进度条走到 **100%（6.0 MB / 6.0 MB）** |
| macOS 授权提示 | 系统弹出「"loki" 想访问可移除宗卷上的文件」，点允许后继续（见下方说明） |
| 安装完成 | 应用自己重启；`Info.plist` 的版本变为 **0.9.0**，进程 PID 也变了 |

**结论**：`latest.json` 解析、版本比较、公钥加载、签名校验、包替换、重启，全链路通。

**关于那个授权提示**：macOS 在应用要替换自己所在的 `.app` 时会问一次"访问可移除宗卷上的文件"。这是系统的 TCC 提示，不是应用要权限；点「允许」即可。若用户点了「不允许」，更新会失败并显示错误，不会留下半装状态。

> 注意：这次验证用的是本地构建的 0.8.9（无签名产物），**只有下载下来的那个包需要签名**，它带 CI 的 minisign 签名，用配置里的公钥校验通过。

### 1.9 两个平台差异

- **macOS**：更新包是 `.app.tar.gz`，插件解压替换 `loki.app`，随后 `app.restart()` 重新执行。**注意**：ad-hoc 签名的 app 替换后 Gatekeeper 状态可能变化，若用户遇到"已损坏"，仍需 `xattr -dr com.apple.quarantine`。
- **Windows**：更新包是 NSIS 安装器，插件以静默参数运行它，安装器负责关掉旧进程并重启。`installer_args` 用默认值即可。

---

## 二、Windows 安装界面语言

### 2.1 结论

Tauri 的 NSIS 安装器本身支持多语言，且**默认按系统语言自动选择**（`$LANGUAGE` 由系统区域决定），不需要额外代码。

配置（`src-tauri/tauri.conf.json` → `bundle.windows.nsis`）：

```json
{
  "languages": ["SimpChinese", "English"],
  "displayLanguageSelector": false
}
```

两处细节：

1. **顺序有意义**。系统语言没有匹配项时，NSIS 回退到**列表里的第一个**。把 `SimpChinese` 放首位，未知区域会显示中文——正好符合「实现不了就只用中文」的兜底要求。
2. `displayLanguageSelector: false` 表示不弹语言选择框，纯自动。若想给用户手动切换，改成 `true`。

### 2.2 为什么只放了两种语言

`languages` 的取值是 NSIS MUI 语言名（`SimpChinese`、`English`、`TradChinese`、`Japanese`、`Korean`、`German`、`French`、`Spanish`、`Russian`、`Portuguese`、`Italian` 等）。Tauri 文档明确示例支持 `English` 与 `SimpChinese`；其余名称我**无法在当前环境（macOS，且无网络）核实 Tauri 打包时是否随附对应 `.nlf` 语言文件**。

写错名字会导致 **Windows 打包直接失败**，而这里没有 Windows 环境可以验证。所以只放确定可用的两个（中文优先），把扩展方法写在下面，而不是赌一把。

### 2.3 想加更多语言

在 `languages` 里按 NSIS 官方语言名追加即可，例如：

```json
"languages": ["SimpChinese", "English", "TradChinese", "Japanese"]
```

**然后必须在 Windows 上跑一次 `pnpm tauri build`** 确认打包通过、且安装界面语言正确。语言文件名对照 NSIS 发行版的 `Contrib/Language files/`（例如 `SimpChinese.nlf`）。

### 2.4 验证状态

| 项 | 结果 |
|---|---|
| JSON 配置合法、字段名与 `tauri-utils` 的 `NsisConfig` 一致 | ✅（`languages: Option<Vec<String>>`、`display_language_selector: bool`） |
| Windows 安装器实际显示中文/英文 | ❌ 未验证（需 Windows 构建） |

---

## 三、留给后续的待办

1. 用一次真实的 tag 发布，完整验证「检查 → 下载 → 校验 → 安装 → 重启」。
2. 若仓库保持 private，换成公开的 `latest.json` 托管地址。
3. 生成带口令的签名私钥会更稳妥（当前为空口令）；换钥后旧的已发布版本将无法自动升级，需要手动过渡一次。
4. 更新检查目前只在启动时跑一次；如需"每天检查"，在 `App.tsx` 里加一个定时器复用 `useUpdates.getState().check(true)`。
5. Windows 安装器语言若要扩展，按 §2.3 在 Windows 上验证。
