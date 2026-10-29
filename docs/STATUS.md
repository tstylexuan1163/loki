# loki — 构建进度

> **当前版本 v0.9.0**。打包产物：`src-tauri/target/release/bundle/dmg/loki_0.9.0_aarch64.dmg`
> 与 `.../macos/loki.app`（ad-hoc 签名，arm64）。v0.8.0 的新增内容见第 13 节；
> AI 服务商改造的详细说明单独写在 [AI_PROVIDER_SETTINGS.md](AI_PROVIDER_SETTINGS.md)。

> 基于对 [Hatoba](https://github.com/scarletkc/Hatoba) 架构的分析蓝图重新实现（蓝图不在本仓库中）。
> 本文记录**实际已验证**的能力，与"写了但没跑过"的部分严格区分。

## 一、已验证可用（真机跑过）

截图见 [screenshots/](screenshots/)。

| # | 能力 | 证据 |
|---|---|---|
| 1 | 应用启动、窗口、自定义标题栏、深色主题 | [01-onboarding.png](screenshots/01-onboarding.png) |
| 2 | 主密码设置 + 实时强度指示 | [02-filled.png](screenshots/02-filled.png) |
| 3 | Argon2id(m=64MiB,t=3,p=4) 派生 → 创建保险库（实测约 4.4s） | 日志 + [03-hosts-empty.png](screenshots/03-hosts-empty.png) |
| 4 | 主机 CRUD（名称/地址/端口/用户名/密码/标签） | [04](screenshots/04-host-editor.png) · [05](screenshots/05-host-filled.png) · [06](screenshots/06-host-list.png) |
| 5 | **本地库只存密文** | 见下方"安全验证" |
| 6 | 锁定 / 解锁 | [07](screenshots/07-locked.png) · [09](screenshots/09-unlocked.png) |
| 7 | 错误主密码被拒绝（中文提示） | [08](screenshots/08-wrong-password.png) |
| 8 | 重启应用后数据可正确解密读回 | 解锁 → 主机列表完整恢复 |
| 9 | 15 个 Rust 单元测试全绿 | `cargo test` |

### 安全验证（关键）

创建保险库并存入一台含密码的主机后，直接读 s​qlite：

```bash
sqlite3 ~/Library/Application\ Support/app.loki.desktop/loki.db "SELECT id, kind, envelope FROM items;"
```

结果只有一条密文信封：

```json
59c8d54e-...-70a81b67c5fb | host | {"v":1,"n":"G9QFlCyWYs/irsFg","c":"UbMCuPw8Rn+XebZKy..."}
```

对全库 `.dump` 做明文检索：

| 明文 | 命中 |
|---|---|
| `prod-api-tokyo` | **0** |
| `10.0.12.31` | **0** |
| `deploy` | **0** |
| `SuperSecret123!` | **0** |
| `k8s` | **0** |

即：**主机名、IP、用户名、密码、标签全部落库前已加密**，直接读数据库文件拿不到任何明文。

## 二、SSH 终端：已打通 ✅

M1 的核心目标达成，端到端验证过：

```
user@mac ~ % echo LOKI_UI_$((40+2))
LOKI_UI_42
```

真实 shell 提示符、命令回显、远端对 `$((40+2))` 求值——输入输出双向都通。

### 后端（集成测试，`src-tauri/tests/ssh_live.rs`）

| 能力 | 测试 |
|---|---|
| 主机指纹与 `ssh-keygen -lf` 逐字符一致 | `first_contact_reports_host_key` |
| 未知主机被拒绝，不静默信任 | 同上 |
| 钉住指纹 + 公钥认证 | `connects_and_runs_a_shell_with_a_pinned_key` |
| PTY + shell + 写入 + 输出回传 | 同上（`LOKI_MARK_$((6*7))` → `LOKI_MARK_42`） |
| 晚挂载面板的 backlog 回放 + 取走即清空 | 同上 |

### UI（真机跑过）

- 主机密钥确认弹窗，指纹与实际一致
- 连接后 xterm 显示登录 banner 与真实提示符
- 键盘输入到达远端、输出回显

### 修掉的两个真 bug

1. **输出丢失**：原先由 store 持有 writer，`TerminalPane` 挂载前收到的数据无人接收，屏幕全黑。
   改为 **pane 自己订阅 `term-event` 并按 sessionId 过滤**，同时在 Rust 侧缓存输出、挂载时 `term_backlog` 取走回放（取走即清空，避免重复）。
2. **键盘无响应**：xterm 的隐藏 textarea 需要显式 `term.focus()`，且要在面板内任意处点击时重新聚焦。

另修：Tauri 2 capabilities 缺 `core:event:allow-listen`（否则 `listen` 直接报权限错误）、`HostInput` 缺 `rename_all = "camelCase"`（私钥被静默丢弃）、编辑主机时密码被清空。

### 说明

验证过程中我的自动化脚本反复与系统「聚焦搜索」抢焦点，导致后续几次输入验证不可靠——那是脚本环境的问题，不是 loki 的缺陷。上面的成功记录是焦点正确时取得的。

## 三、M2：恢复码 / 自动锁定 / 密钥管理 ✅

### 恢复码

创建保险库时生成一次性恢复码（128 位 + 4 字节校验和 → 32 位 Crockford Base32，8 组 × 4）：

```
NHG8 980J ADBD GH83 F8M4 M2BG R8CE MWRF
```

- 只显示一次，必须勾选"已保存"才能进入应用；**本机不保存它**。
- 忘掉密码时用「忘记密码？」→ 输入恢复码即可解开保险库。
- 输入容错：大小写、空格、连字符都不影响；I/L→1、O→0；**U 被拒绝**（避免误认）。
- 校验和让输错一个字符在解密前就被拒绝。

真机验证：创建 → 记录 → 锁定 → 用恢复码解锁 → 进入主界面 ✅

### 自动锁定

- 设置页可选 从不 / 1 / 5 / 15 / 30 / 60 分钟（默认 15）。
- 活动追踪用模块级时间戳，鼠标移动不会触发整个应用重渲染。
- 定时器每 5 秒检查一次；**窗口重新可见时立即检查**，所以合盖休眠后回来不会仍处于解锁状态。
- 锁定时同时断开所有 SSH 会话，并清空内存中的主机明文、终端标签、恢复码。

### 密钥管理

- 生成 Ed25519 / RSA 4096，名称自动带日期，可设注释。
- 列表显示 `SHA256:` 指纹，公钥可一键复制，**私钥从不下发到前端**。
- 私钥与主机凭据一样以密文存进同一张 `items` 表。

真机验证：生成 Ed25519 → 列表显示指纹 ✅

### 修改主密码

设置页可改密码：先验证当前密码，再重新派生。只重新加密保险库密钥，**不重写任何条目**（有测试断言密文逐字节未变），恢复码继续有效。

真机验证：改密 → 旧密码被拒（「主密码错误」）→ 新密码解锁成功 ✅

后端测试：22 个单元测试全绿，覆盖恢复码往返、容错、错位拒绝、错误恢复码被拒、改密不重写条目。

## 四、M3：SFTP + 端口转发 ✅

### SFTP

终端标签上的「文件」按钮打开侧栏，与终端共用同一个 SSH 会话，不额外登录。双击目录进入，双击文件在内置编辑器中打开（4 MB 上限）。

真机验证：列出主目录 → 进入 `.cargo` → 显示真实文件与大小（`env 300 B`、`.global-cache 184.0 KB`）。

集成测试 `sftp_lists_and_round_trips_a_file` 覆盖：写入新文件 → 出现在列表且大小正确 → 读回内容一致 → 重命名 → 删除后消失。

**抓到的 bug**：`SftpSession::write` 只开 `WRITE` 标志，新建文件必然失败（"No such file"）。改成显式 `WRITE|CREATE|TRUNCATE`。

### 端口转发

设置本地端口 → 远端主机:端口，或 SOCKS5。列表显示实际绑定端口，可停止。

真机验证：本地起 HTTP 服务在 18080，转发 `8080 → 127.0.0.1:18080`，`curl http://127.0.0.1:8080/` 返回 **HTTP 200 + 真实 HTML**，界面显示 `本地 · 127.0.0.1:8080 → 127.0.0.1:18080`。

集成测试 `local_forward_carries_traffic` 同样验证 HTTP 响应穿过隧道。

SOCKS5 握手已实现（no-auth + CONNECT），**未做端到端验证**。

## 五、M4：AI 助手 ✅

终端页「助手」按钮打开侧栏。支持两种协议：**OpenAI 兼容**（OpenAI / OpenRouter / Ollama / LM Studio / vLLM）和 **Anthropic**。设置页有 5 个预设。

### 工具与权限

模型可调用：`read_terminal`、`run_command`、`list_directory`、`read_file`、`write_file`、`grep`。

三种权限模式：

| 模式 | 行为 |
|---|---|
| 每次确认（默认） | 所有工具调用都要点「允许」 |
| 自动 | 只读工具直接跑；改状态的仍需确认 |
| 完全放开 | 全部直接跑 |

工具代码在 Rust 里，审批也走 Rust 通道；面板关闭会被当作「拒绝」而不是静默执行。

### 真机验证（用本地 mock 模型服务）

1. 发「看看磁盘占用」
2. 模型回复并请求 `run_command {"command": "df -h /"}`
3. 工具卡显示「等待确认」+ 参数 + 允许/拒绝按钮
4. 点「允许」→ 工具卡变「完成」
5. 模型拿到结果后总结：「工具返回了 **149 个字符**」

**交叉验证**：本地执行 `df -h /` 输出恰好 **149 字节**，与模型收到的长度一致；展开工具卡显示的是真实 `df` 输出（`/dev/disk3s1s1 228Gi 13Gi 51Gi 20% /`）。这证明命令真的在远端执行了。

## 六、M5：同步（Worker + D1，仅存密文）✅

`workers/sync/` 是一个可部署到用户自己 Cloudflare 账号的 Worker + D1。

**服务端不做任何加解密**：`envelope` 就是客户端本地已加密的 blob，Worker 只存它和版本号。

| 端点 | 用途 |
|---|---|
| `GET /v1/health` | 探测，含 `initialized` |
| `GET /v1/prelogin` | 取 KDF salt |
| `POST /v1/setup` | 首次初始化（只能一次） |
| `POST /v1/login` | 换 token |
| `GET /v1/items?since=N` | 增量拉取 |
| `PUT /v1/items` | 批量推送 |

冲突规则：按明文 `updated_at` 做 LWW，平局保留服务端版本；服务端强制执行同一规则，过期的推送被拒绝。

会话 token 只存 SHA-256 哈希，D1 被读也无法重放。

### 端到端测试（`tests/sync_live.rs`）

用 Node + SQLite 模拟 D1，验证：

- 条目以密文传输（断言 `prod-api-tokyo`、`10.0.12.31` 不出现在载荷中）
- 拉回的信封**能正确解密**回原始明文
- **过期写入被拒绝**（LWW 生效）

**抓到的 bug**：Worker 与测试服务的 `items` 表都缺 `kind` 列，而客户端需要它区分条目类型——部署后同步会坏。已补齐并贯穿读写路径。

## 七、v0.6.0：界面改造 / Telnet / 串口 ✅

### 1. 日间与夜间模式

`:root[data-theme="light"]` 提供一套暖白配色，强调色降到 `#0f9d6e` 以在白底上保持可读；**终端在两种主题下都保持深色**，因为浅色终端在高对比场景下看不清。

三档：日间 / 夜间 / 跟随系统。选择持久化到本地，`initTheme()` 在首次渲染前执行，窗口不会闪错主题。跟随系统时会监听 `prefers-color-scheme` 变化。

真机验证：设置 → 外观 → 三张带预览的卡片；点「日间」整个界面立即变暖白，主界面层次依然清晰。

### 2. 布局参照 Hatoba

- 侧栏按 Hatoba 分组：**主机 / 端口转发 / 密钥库**，底部是**设置 / 锁定**
- 设置从整页改成**模态窗口**，六个 tab（通用、外观、安全、AI、同步、关于），内容是分组卡片
- 主机列表增加**卡片 / 列表两种视图**和搜索框（搜名称、地址、用户、标签）
- 卡片显示名称、`user@host:port`、标签色块、协议徽章

### 3. Telnet

- 完整 IAC 协商：接受 ECHO / SUPPRESS GA / NAWS，其余一律拒绝（RFC 854 允许）
- `IAC IAC` 还原为字面 0xFF；子协商内容被消费而不上屏
- 连接时先发 NAWS 告知窗口大小
- 与 SSH 共用同一条 `term-event` 通道和同一个终端组件

真机验证：对本地测试服务器连接，屏幕显示 `loki-telnet-test ready` 与提示符，协商字节没有泄漏成乱码。

### 4. 串口

- 用 `serialport` 枚举本机串口（实测列出 4 个设备）
- 波特率 300–230400，数据位 5–8，校验 无/奇/偶，停止位 1/2
- 读取在独立 OS 线程（底层读是阻塞的），写入走 `try_clone` 的句柄
- 同样接进 `term-event` 通道

**尚未真机验证**：串口的实际收发需要真实串口设备或 `socat` 虚拟串口，当前只验证了枚举与打开路径。

### 抓到的三个真 bug

1. **Telnet 被要求提供 SSH 凭据** —— `session_open` 在读主机后立刻构造凭据，导致未保存密码的 telnet 主机报「这台主机还没有保存密码或私钥」。已改成先按协议分派，只有 SSH 才要求凭据。
2. **Telnet 读取空转** —— 协商阶段 `strip_commands` 返回空数组，循环零延迟继续，实测一秒钟跑了几十万次，把 tokio 运行时饿死。空数据时必须 `yield_now()`。
3. **`term_backlog` 只认 SSH 会话** —— telnet/串口的 sessionId 查不到，前端收到错误又被 `.catch(() => {})` 吞掉，表现为终端永远空白。已改为同时查询两个管理器。

## 八、v0.7.0：补全功能 + 界面重做 ✅

### 新增功能

| 能力 | 实现 | 验证 |
|---|---|---|
| 已知主机（TOFU） | 指纹以 `known_host` 条目加密存进保险库；已信任的直接连，新主机弹确认，**同类型密钥变化**单独红色警告并显示新旧指纹；设置 → 安全 可查看/忘记 | `chain_live`：首次询问→接受→记录、已知直接连接、密钥变化报告旧指纹 |
| 认证链 | 主机私钥 → 密钥库中的密钥 → ssh-agent → 密码；都不行时弹出密码框（可选“记住密码”）；密码被关闭的 PAM 服务器走 keyboard-interactive | `chain_live`：无凭据返回 needPassword；agent 认证（临时 agent，`LOKI_TEST_AGENT=1`） |
| ssh-agent | 先读 `SSH_AUTH_SOCK`，Finder 启动时再问 `launchctl getenv` | 同上 |
| 跳板机 ProxyJump | 先登录跳板机，再在它的 direct-tcpip 通道上做第二次握手；两跳都走 TOFU | `chain_live`：同一台 sshd 作自己的跳板，`echo via-jump-42` 回显正确 |
| 本地终端 | portable-pty 启动登录 shell（`-l`），与 SSH 共用终端组件 | 单元测试真跑 shell：`LOKI_42` 回显、退出码 3 |
| 快速连接 | ⌘K 面板：输入 `user@host:port` 直接连，或搜索已保存主机、最近连接、本地终端 | 模拟界面 |
| SFTP 传输 | 上传/下载文件和**整个目录**、进度与取消、下载先写 `.part` 再改名、拖拽上传、递归删除（拒绝删 `/`） | `chain_live`：600 KB 目录上传→下载→逐字节比对→递归删除 |
| SFTP 面板 | 面包屑、筛选、显示/隐藏点文件、右键菜单（打开/下载/重命名/复制路径/删除）、新建文件/文件夹、编辑器 ⌘S | 模拟界面 |
| 服务器状态栏 | 一段 POSIX sh 脚本，Linux 读 `/proc`，macOS 用 `sysctl/vm_stat`；CPU、内存、磁盘、网络速率、负载、运行时间，5 秒一次 | 单元测试在本机跑脚本；`chain_live` 经 SSH 采样 |
| 密钥导入 | OpenSSH / PEM，带口令；从文件读取时由 Rust 读，私钥不经过 WebView；按指纹去重；重命名 | `cargo test` |
| `~/.ssh/config` 导入 | 解析 Host/HostName/User/Port/IdentityFile/ProxyJump，`Host *` 作为默认值；私钥文件导入密钥库，需要口令的留给 agent | 单元测试 |
| 主机管理 | 删除（同时清掉别的主机对它的跳板引用）、复制（含加密的凭据）、登录后执行命令、显式“移除已保存密码/私钥” | `cargo test` + 模拟界面 |
| 终端设置 | 字体、字号、行高、光标样式/闪烁、回滚行数、选中即复制、状态栏开关，实时生效 | 模拟界面 |

### 修掉的 bug

1. **切换标签或页面会清空终端**：之前只挂载当前标签的 xterm，切走就销毁，屏幕和回滚全丢。现在每个标签一个常驻 xterm，非活动的用 `visibility: hidden` 隐藏；终端页本身也常驻挂载。
2. **每次连接都要重新确认指纹**：指纹从未保存。现在存进保险库。
3. **AI 的 `read_terminal` 读到的是登录 banner 而不是当前屏幕**：输出缓冲只存“连接后的前 256 KB”。拆成两块：一次性交给面板的 `pending`，和给助手用的滚动 64 KB `tail`。
4. **`exec_command` 截断输出会 panic**：`String::truncate` 落在多字节字符中间会 panic，中文输出必触发。改为回退到字符边界（有单元测试）。
5. **关闭会话不清理转发和 SFTP 缓存**：现在 `session_close` 一并停止该会话的转发、丢弃 SFTP 通道。
6. **锁定只断开 SSH**：Telnet、串口、本地终端、转发、传输现在都一起关闭，前端标签也同步清空。
7. **自动锁定时间不持久**：每次启动重置为 15 分钟，现在存进本地偏好。
8. **串口回放缓冲无上限**：改为 256 KB 封顶。
9. **主机列表没有删除入口**：API 有但 UI 没有。
10. **PTY 没有下发终端模式**：部分服务器上 Ctrl-C、UTF-8 行编辑异常；现在显式发送 cooked 模式与 `IUTF8`。另加 30 秒 keepalive。

### 界面

- 侧栏改为带文字的导航 + **会话列表**（随时跳回某个终端），底部设置/锁定。
- 标题栏中间是全局搜索 / 快速连接入口（⌘K）。
- 主机卡片：彩色首字母头像、在线会话数徽章、认证方式图标、悬停出现“连接”；双击即连接；右键菜单；按标签筛选。
- 终端：浏览器式标签（中键关闭、双击重命名、右键菜单）、连接中/指纹确认/密码/失败的覆盖层、断开后的“重新连接”条、可拖拽宽度的侧面板、查找栏。
- 全部图标改为内联 SVG（之前是 Unicode 符号，字重和大小不一）。
- 统一的模态框、确认框、提示条（替代原来顶在页面上方的错误条）、开关、分段控件。
- 日间主题下激活标签的对比度单独处理（标签与深色终端相连）。

### 验证方式与限制

- 后端：65 个单元测试 + 12 个实机集成测试（`ssh_live` 4 个、`chain_live` 8 个），全部通过；clippy `-D warnings` 无警告。
- 实机测试用的 sshd：用户权限运行 `/usr/sbin/sshd -D -f <config>`，配置要点：`Port 2222`、`ListenAddress 127.0.0.1`、独立 `HostKey`、`AuthorizedKeysFile` 指向测试公钥、`UsePAM no`、`StrictModes no`、`Subsystem sftp /usr/libexec/sftp-server`。
- 前端：`dev/mock.html` 在浏览器里接一个内存假后端，逐页检查过主机列表、编辑器、终端标签切换、指纹确认、密码提示、文件面板、下载进度、AI 面板、密钥库、转发、设置、日间主题。
- 后端单元测试现为 67 个（新增 TermEvent 字段名、私钥规范化两个回归测试）。

### 真机窗口验证（release 版 loki.app + 临时保险库 + 本机测试 sshd）

| 步骤 | 结果 |
|---|---|
| 创建保险库、解锁 | ✅（主密码由用户手输：macOS 安全输入不允许自动化键入密码框） |
| 密钥库 → 导入私钥 → 原生文件对话框选文件 | ✅ 指纹与 `ssh-keygen -lf` 一致；私钥由 Rust 读取 |
| 新建主机，选用密钥库中的密钥 | ✅ |
| 首次连接 → 指纹确认 → 连接 | ✅ 指纹与测试 sshd 主机密钥一致 |
| 重启应用后再次连接 | ✅ 不再询问指纹（已持久化） |
| 终端双向输入输出、UTF-8 中文 | ✅ `LOKI_42`、`中文 ok` |
| 状态栏 CPU / 内存 / 磁盘 / 负载 | ✅ |
| ⌘T 面板 → 本地终端 | ✅ `/bin/zsh`，文件按钮对本地标签禁用 |
| SSH ↔ 本地标签来回切换 | ✅ 屏幕内容保留 |
| 文件面板：根目录、进入 `/etc` 符号链接、筛选 | ✅ |
| 右键 → 下载到… → 原生保存对话框 | ✅ 与 `/etc/hosts` 逐字节一致，传输栏显示“完成” |
| 上传到无写权限目录 | ✅ 传输栏显示“无法写入 /etc/hosts：Permission denied” |
| 设置 → 安全 → 已信任的主机密钥 | ✅ 列出 `127.0.0.1:2222` |
| 设置 → 终端 → 字号 | ✅ 打开的终端实时重排 |
| ⌘L 锁定 | ✅ SSH 断开（sshd 日志可见）、本地 shell 进程结束、回到解锁页 |

真机验证抓到并已修复的问题：

1. **终端收不到任何实时输出**（严重，旧版就存在）：`TermEvent` 上的 `#[serde(rename_all = "camelCase")]` 只改变体名，不改字段名，后端发的是 `session_id`，前端按 `sessionId` 过滤，所有输出帧都被丢弃。旧版靠回放缓冲把一部分输出补上，问题被掩盖；浏览器模拟环境直接发 `sessionId`，所以也没暴露。加 `rename_all_fields` 并补序列化回归测试。
2. **粘贴的私钥被 macOS “智能破折号”破坏**：`-----BEGIN` 里的连字符被自动替换成 `—`，导致无法解析。后端统一重建 PEM 头尾（也处理 `\r` 和首尾空格），代码类文本框和搜索框关闭拼写检查/自动更正。
3. **标题栏占两行**：`titleBarStyle: Transparent` 时 WebView 不延伸到原生标题栏，自绘标题栏被挤到下面且左侧空着红绿灯位置。改为 `Overlay` + `trafficLightPosition`，去掉写死的深色窗口主题，并补上 `core:window:allow-start-dragging` 权限。
4. 串口主机的 `data_bits` / `stop_bits` 在前端读成了 `dataBits` / `stopBits`，编辑时总显示默认值。

仍未在真机窗口里验证：Finder 拖拽上传（需要另外授权访达）、ssh-agent 经 `launchctl` 的路径（集成测试覆盖了 agent 认证本身）、AI 助手（没有配置模型服务商）。

- 远程转发（-R）未实现：russh 0.54 的 `tcpip_forward` 需要 `&mut Handle`，而会话句柄是共享的 `Arc`。
- 串口实际收发、SOCKS5 端到端仍未用真实设备验证。

## 九、与蓝图的一致性

实现严格遵循蓝图中的三条硬约束：

1. **秘密单向**：`commands.rs` 的 `HostView` 只暴露 `has_password: bool`，没有任何命令返回密码或密钥。`Vault::encrypt_item` 是唯一出口。导入私钥文件时由 Rust 读取，内容不进 WebView。
2. **只存密文**：`store.rs` 只接受已加密的 `Envelope`；`vault.rs` 的测试 `items_are_encrypted_at_rest` 断言序列化后的密文不含明文。已知主机同样以密文存放。
3. **密文绑定身份**：AAD = `loki/item/v1/{id}`，测试 `item_ids_are_bound_by_aad` 断言把密文挪到另一个 id 下会解密失败。

## 十、已知问题

- **未经第三方审计**，不建议存放生产凭据。
- **pnpm 严格校验误报**：pnpm 12 的 `ERR_PNPM_IGNORED_BUILDS` 会因为 esbuild 的 postinstall 报错，但依赖实际已安装，构建与运行正常。
- `russh 0.54` 会触发一条 future-incompat 警告（上游 crate 的问题）。
- 前端 bundle 约 680 KB（xterm 占大头），Vite 会提示 chunk 过大，不影响桌面应用。

## 十三、v0.8.0：AI 服务商设置改造 ✅

界面按 Hatoba 的服务商对话框重做，并把「只能接几家官方预设」改成任意端点。

### 改了什么

| 项 | v0.7 | v0.8 |
|---|---|---|
| 协议 | 2 个硬编码值 | 3 个：Chat Completions / Responses / Anthropic |
| 模型 | 单个字符串 | **有序列表**，可手动加、可一键拉取 |
| 基础 URL | 后端偷偷拼 `/v1` | 显式包含版本，`/api/v1` 等也认 |
| 拉取模型 | 无 | 「获取模型列表」→ 可点击 chip 添加 |
| 连接测试 | 无 | 「测试连接」→ 真实发一条 ping |
| 空密钥 | 无条件发空 Bearer | **不发送认证头**（本地服务器可用） |
| URL 校验 | 无 | 拒绝非 http(s)、无主机、URL 内嵌账号密码 |

### 真机验证

对本地 mock 端点（`http://127.0.0.1:18090/v1`，返回 3 个模型）：

1. 「获取模型列表」→ 列出 `mock-embed` / `mock-gpt-4o` / `mock-gpt-4o-mini`，提示「服务商列出 3 个模型」
2. 点选加入模型列表
3. 「测试连接」→ **`连接正常，模型回复：pong（模型 mock-gpt-4o-mini 可用）`**
4. 保存 → 列表行显示 `OpenAI Chat Completions · http://127.0.0.1:18090/v1` 与 `1 个模型：mock-gpt-4o-mini`
5. 对 `loki.db` 全量检索 `mock-gpt-4o-mini` / `127.0.0.1:18090` / `Mock 本地`：**均 0 命中**（仍是密文）

### 未验证

- `anthropic` 协议本轮没有重新真机测试（代码路径与 v0.7 相同）
- `responses` 协议只有请求体/响应解析的单元测试，**没有可用端点做真机验证**
- 老库（v0.7 结构）迁移有兼容分支与单测，但未用真实旧库回放

`cargo test` 74 个通过，clippy `-D warnings` 无警告，`tsc` 通过。

## 十四、v0.8.1：Windows 标题栏合并 ✅（待 Windows 验证）

Windows 上原来有两条标题栏：系统原生一条 + 应用自绘一条。

- 根因：`titleBarStyle` / `hiddenTitle` 是 macOS 专有字段，Windows 上被忽略，窗口保留完整原生装饰。
- 改法：新增 `src-tauri/tauri.windows.conf.json`（`decorations: false`），并把窗口对象写全（平台配置里的 `app.windows` 数组是**整体替换**）；新增 `src/app/WindowControls.tsx` 自绘最小化/最大化/关闭，仅在非 macOS 渲染。
- 细节与风险见 [WINDOWS_FRAMELESS.md](WINDOWS_FRAMELESS.md)。

macOS 未改配置，行为不变。**Windows 实机效果未验证**（开发机为 macOS，无法构建 Windows 包）。

## 十五、v0.9.0：检查更新 + 安装器语言 ✅（部分待真机验证）

### 检查更新

- 启动约 4 秒后静默检查一次；有新版才提示，失败不打扰。
- 设置 → 关于 有「检查更新」按钮与状态文字；发现新版时右下角浮出提示条（非模态，不打断终端）。
- 点「下载并安装」显示进度；失败可重试。
- 走官方 `tauri-plugin-updater` 的 **Rust API**，自己包了两条命令，**不需要 npm 包**。
- 更新包强制 minisign 签名校验；`createUpdaterArtifacts` 只放在 `tauri.release.conf.json` 供 CI 使用，本地构建不需要私钥。

### 安装器语言

`bundle.windows.nsis.languages = ["SimpChinese", "English"]`，`displayLanguageSelector: false`。NSIS 按系统语言自动选择，未匹配时回退到第一个（中文）。

### 验证

| 项 | 结果 |
|---|---|
| 自动检查 → 提示条（版本 + 说明） | ✅ 用本地假 `latest.json`(9.9.9) 实测 |
| 关于页手动检查 → 显示新版本 + 安装按钮 | ✅ 实测 |
| 下载失败/重试 UI | ✅ 实测 |
| **签名校验拦下伪造包** | ✅ `Invalid encoding in minisign data`，未安装任何文件 |
| 真实发布包完整升级流程 | ❌ 未验证（需真实 Release + 签名产物） |
| Windows 安装器语言 | ❌ 未验证（需 Windows 构建） |

细节见 [UPDATES.md](UPDATES.md)。
