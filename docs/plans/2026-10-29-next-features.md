# 下一批功能：评估、计划与执行提示词

> 写于 2026-10-29，基于 v0.10.1（`90f8dcb`）。给执行者（DeepSeek harness）用：先读「通用约束」，再按第二节的顺序做（4 → 3 → 2 → 1），每项一个独立提交。

## 一、需求评估

| # | 原始描述 | 实际要的是什么 | 规模 | 风险 |
|---|---|---|---|---|
| 1 | AI 会话增加上传/选择文件（上传 SSL 证书让它更新、上传脚本让它执行） | **把本机文件交给 AI 在服务器上使用**：附加文件时立即通过 SFTP 传到服务器暂存目录，AI 拿到远端路径后用现有工具完成安装、重载、执行。重点不是"让模型读文件"，而是"让 AI 能用这个文件" | 中–大 | **高**：证书常带私钥，绝不能发给模型服务商；后续改动服务器的命令照常走确认 |
| 2 | AI 会话增加历史会话记录 | 对话持久化：可查看、继续、重命名、删除过去的对话，按主机区分 | 中 | 中：对话里有服务器输出，必须加密存保险库；不能拖慢同步 |
| 3 | 终端增加记录会话导出（如导出交换机配置） | 两件事：**① 导出当前终端内容**（回滚缓冲区 → 文本文件）；**② 会话录制**（从某一刻起把输出持续写入文件，长配置超出回滚行数也不丢） | 中 | 低–中：写文件的路径必须由用户在原生对话框里选，不能由 WebView 任意指定 |
| 4 | 标签页加号的快速连接可选 SSH / Telnet / 串口 | 快速连接面板加协议选择：Telnet 填地址端口，串口选设备和波特率，不用先建主机 | 小 | 低：后端 `telnet_open` / `serial_open` 已存在 |

### 对描述的几处澄清（按这个理解执行）

- **第 1 项**：采用"附加即上传"：用户附加文件 = 同意传到当前服务器，loki 立即传到 `~/.loki/uploads/…` 暂存目录（权限 700/600）并校验哈希；不新增 AI 工具，安装、替换、重载、执行都由 AI 用现有的 run_command 完成，照常走确认。小的纯文本文件（脚本、配置）默认让模型看到内容，方便它先说明脚本会做什么；**私钥只给路径、大小和哈希，内容永远不发给模型**，并且所有工具输出都会过滤私钥块（防止 AI `cat` 私钥）。只支持 SSH 标签。
- **第 3 项**：交换机配置通常很长，并且会分页（`--More--`）。录制功能比导出回滚内容更可靠；界面上要提示先关闭分页（Cisco `terminal length 0`，华为/H3C `screen-length 0 temporary`）。
- **第 2 项**：历史只保存在本机保险库，**不参与云同步**（体积大、价值低，而且会把服务器输出带到云端）。

## 二、执行顺序与理由

1. **第 4 项 快速连接选协议**：最小，独立，先热身熟悉代码。
2. **第 3 项 终端导出与录制**：独立于 AI，改动集中在终端与输出管道。
3. **第 2 项 AI 历史会话**：先把对话持久化做好，第 1 项的附件元数据可以直接存进去。
4. **第 1 项 AI 附件**：安全要求最高，放在最后，做的时候前面的代码已经熟悉。

第 2、3 项互不依赖，可以并行。第 1 项不依赖第 2 项，但两项都改 `src/features/ai/AiPanel.tsx`，不要同时做。

## 三、通用约束（每项都适用）

把这一段放在每个提示词前面（下面的提示词已经包含"先读通用约束"）。

```
工作目录 /Volumes/loki/REA/loki，分支 main。开始前 git pull。

项目：loki，Tauri 2 + React 19 + TypeScript + Zustand 前端，Rust 后端（russh、tokio、rusqlite）。
先读 README.md、docs/STATUS.md 的最后几节，了解已有约定。

必须遵守：
1. 秘密只在 Rust 进程：密码、私钥、API Key、文件内容里的私钥，绝不传到 WebView、日志或模型。
   需要读本机文件时，由 Rust 按路径读取，不在前端用 fs 读。
2. 持久化数据一律用 vault.encrypt_item / store.put_item 加密存入 items 表（新 kind 名），
   不得写明文到磁盘或 localStorage（localStorage 只放无害的界面偏好，见 src/app/prefs.ts）。
3. 写本机文件时，路径必须来自用户在原生对话框里的选择。优先在 Rust 里用
   tauri-plugin-dialog 的 Rust API 弹框（app.dialog().file()…），不要提供
   “前端传任意路径 + 任意内容就写入”的命令。
4. Tauri IPC 字段名：结构体用 #[serde(rename_all = "camelCase")]；带 tag 的枚举事件必须同时写
   rename_all = "camelCase", rename_all_fields = "camelCase"（项目里踩过坑：字段名不一致会让前端
   静默丢掉所有事件）。前端 invoke 参数用 camelCase。
5. 界面文案用简体中文；代码注释用英文，风格和现有代码一致：解释"为什么"，不写废话。
   用现有组件（src/components/ui.tsx 的 Modal、Menu、Segmented、Switch、toast、confirmDialog；
   src/components/icons.tsx 的图标），样式写进 src/styles/app.css，用已有 CSS 变量（深浅色主题都要好看）。
6. 新后端逻辑要有单元测试（cargo test）。前端行为要能在模拟界面里演示：更新 dev/mock-ipc.js，
   让 `pnpm dev` 后打开 http://localhost:5273/dev/mock.html 能走通新功能。
7. 完成前必须全部通过：
     cd src-tauri && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
     cd .. && npx tsc --noEmit && npx vite build
8. 在 docs/STATUS.md 末尾新增一节：做了什么、怎么验证的、哪些没验证（如实写）。
9. 一项一个提交，提交信息用英文、说明动机；推送到 main。不要改版本号、不要打 tag、不要发布、
   不要改 .github/workflows/release.yml。
10. 不确定的地方按本文档的设计决定执行；与现有代码冲突时以安全和现有约定为准，并在 STATUS 里说明。
```

---

## 四、提示词

### 第 4 项：快速连接支持 SSH / Telnet / 串口

```
先读 docs/plans/2026-10-29-next-features.md 的「通用约束」，然后完成下面的任务。

目标：标签栏的「+」、侧栏「会话」旁的「+」、⌘T / Ctrl+Shift+T 打开的快速连接面板
（src/features/terminal/QuickConnect.tsx）支持选择协议：SSH、Telnet、串口、本地终端。

现状：
- 面板只能输入 user@host:port 走 SSH，或选已保存主机、最近连接、本地终端。
- 后端已有 telnet_open / serial_open / serial_ports 命令（src-tauri/src/commands.rs），
  前端包装在 src/lib/api.ts 的 directApi。
- src/features/terminal/terminals.ts 的 openDirect() 对非本地标签只会用 hostId 调 session_open，
  没有 hostId 的 telnet/串口标签无法打开，也无法重连。

要做：
1. 面板顶部加协议切换（Segmented：SSH / Telnet / 串口 / 本地）。默认 SSH，行为和现在完全一致；
   上次选的协议记在 prefs（src/app/prefs.ts）。
2. Telnet：输入 host 或 host:port（默认 23），回车连接。最近连接列表按协议区分
   （在 prefs.recentTargets 里用 "telnet://host:port" 这样的前缀区分，旧数据视为 SSH）。
3. 串口：在面板里显示表单：设备下拉（serial_ports，带刷新按钮；也允许手动输入路径）、
   波特率（300…230400，默认 115200）、数据位/校验/停止位（放在“高级”里折叠，默认 8/无/1）。
4. terminals.ts：新增 openTelnet(target) 和 openSerial(opts)。TermTab 增加可选字段 direct，
   保存连接参数；openDirect 在没有 hostId 时用 direct 参数调用 directApi.telnetOpen / serialOpen；
   reconnect 也要支持。标签标题：telnet 用 host:port，串口用设备名 + 波特率。
5. 标签右键菜单增加「保存为主机」：跳转到主机编辑器并预填协议和参数
   （src/app/store.ts 的 Page 类型给 host-edit 加可选 prefill；HostEditor.tsx 读取 prefill）。
   SSH 快速连接的标签也可以用同一个入口。
6. 更新 dev/mock-ipc.js：serial_ports 返回两个设备，telnet_open / serial_open 返回 DirectInfo，
   并像 SSH 一样推一段欢迎文字。

验收：
- 模拟界面里三种协议都能从「+」打开标签；串口表单能选设备和波特率；断开后「重新连接」可用；
  「保存为主机」打开预填好的编辑器。
- 已有 SSH 快速连接、已保存主机、本地终端的行为不变。
- 通用约束第 7 条全部通过；STATUS 新增一节。
```

### 第 3 项：终端导出内容与会话录制

```
先读 docs/plans/2026-10-29-next-features.md 的「通用约束」，然后完成下面的任务。

背景：用户要把交换机、服务器的输出保存成文件（比如 show running-config 的完整配置）。
需要两个功能：① 一次性导出当前终端内容；② 从某一刻开始持续录制会话输出到文件。

入口：标签右键菜单（src/features/terminal/TerminalView.tsx 的 tabMenu），以及标签栏右侧工具区
加一个「更多」菜单，放：导出终端内容…、导出选中内容…、开始录制…/停止录制。SSH、Telnet、
串口、本地终端都要支持。

一、导出终端内容
1. 前端从 xterm 缓冲区取文本：terminalRegistry（src/features/terminal/TerminalPane.tsx）里有
   每个标签的 Terminal 实例。遍历 term.buffer.active，用 translateToString(true)，把 isWrapped 的
   续行拼回同一行，去掉末尾空行。“选中内容”用 term.getSelection()。
2. 新增 Rust 命令 terminal_export(default_name, text)：在 Rust 里用 tauri-plugin-dialog 弹保存
   对话框（默认文件名：主机名-YYYYMMDD-HHMM.txt），用户取消则返回 None；写入 UTF-8、换行用 \n。
   路径不能由前端传入（见通用约束第 3 条）。
3. 提示：回滚行数在 设置 → 终端 → 回滚行数，导出前若已超出会丢失开头，提示用户改用录制。

二、会话录制
1. Rust 侧录制器：在输出管道里写文件，而不是在前端拼。
   - SSH：src-tauri/src/ssh.rs 的 open_shell 里，读取循环在 OutputBuffer.push 处拿到每块输出。
   - Telnet/串口/本地：src-tauri/src/direct.rs 里 push_bounded 的调用点（共 3 处）。
   给 LiveSession 和 direct 的 Entry 各加一个 Arc<Mutex<Option<Recorder>>> 槽位，
   读取循环在 push 的同时写入录制器。
2. Recorder：BufWriter<File>，每次写入后按时间或大小 flush（至少每秒一次），会话关闭、
   锁定保险库（commands.rs 的 close_everything）时自动停止并 flush。
3. 默认“纯文本”：去掉 ANSI 控制序列（CSI、OSC、字符集切换等）、把 \r\n 统一为 \n、丢弃单独的 \r
   和退格造成的回删（简单实现即可，写清楚局限）。可选“原始输出（含颜色控制符）”。
   ANSI 去除写成纯函数并配单元测试（覆盖颜色、光标移动、OSC 标题、\r\n、跨块被切断的转义序列）。
4. 文件开头写一行头信息：主机/目标、开始时间；停止时写结束时间。
5. 命令：session_record_start(session_id, plain: bool) → 在 Rust 里弹保存对话框后开始，
   返回 { path, startedAt }；session_record_stop(session_id) → 返回 { path, bytes }；
   session_record_status(session_id)。
6. 界面：录制中的标签显示红点 REC；终端状态栏（StatusBar.tsx）显示“● 录制中 文件名 · 已写 N KB”，
   点击可停止。开始录制的对话框里提示：交换机先关闭分页（Cisco `terminal length 0`，
   华为/H3C `screen-length 0 temporary`，Juniper `set cli screen-length 0`）。
7. 更新 dev/mock-ipc.js 覆盖新命令。

验收：
- 单元测试覆盖 ANSI 去除和录制器的启动/写入/停止；clippy 无警告。
- 用本机测试 sshd（docs/STATUS.md 里有配置方法）或本地终端实测：录制期间执行
  `seq 1 50000`，停止后文件有 50000 行且不含控制字符；导出功能在回滚范围内内容完整。
- 关闭标签、锁定时录制自动停止，文件可正常打开。
- 通用约束第 7 条全部通过；STATUS 新增一节。
```

### 第 2 项：AI 历史会话

```
先读 docs/plans/2026-10-29-next-features.md 的「通用约束」，然后完成下面的任务。

目标：AI 助手的对话能保存、查看、继续、重命名、删除，并按主机区分。

现状：src/features/ai/AiPanel.tsx 把对话放在组件 state 里，关掉就没了。一轮回复是按顺序的
parts（文本块 / 工具调用），turnToMessages() 能从 parts 重建发给后端的消息。

存储（Rust）：
1. 新 kind "ai_conversation"，用 vault.encrypt_item 加密存 items 表。payload：
   { id, title, host_id, host_label, provider_id, model, created_at, updated_at,
     turns: [{ role, parts, error }] }。parts 结构和前端一致。
2. 保存时截断：每个工具结果最多 8 KB（超出部分标注“…已截断”），单个对话最多 200 轮；
   总数超过 200 个对话时删除最旧的。
3. 只存本机、不参与同步：src-tauri/src/sync.rs 推送时跳过 kind == "ai_conversation"，
   拉取时也忽略这个 kind。写单元测试证明它不会被推送。
4. 命令：ai_conversations_list(host_id?: string) → 摘要（不含 turns）：id、title、host_label、
   updated_at、turn_count；ai_conversation_get(id)；ai_conversation_save(conversation)
   （upsert，id 由前端生成 uuid）；ai_conversation_rename(id, title)；ai_conversation_delete(id)。
   全部要求保险库已解锁。

界面（AiPanel.tsx）：
1. 面板头部加「新对话」（+）和「历史」（时钟图标）按钮。历史打开一个覆盖在面板内的列表：
   按 今天 / 昨天 / 更早 分组；顶部切换“当前主机 / 全部”；可按标题搜索；每行有重命名、删除
   （删除要确认）。
2. 点开历史对话：载入 turns，可直接继续提问（history() 从 parts 重建消息）。
3. 自动保存：发出用户消息时保存一次，回合结束（done / error）再保存一次。
   标题默认取第一条用户消息的前 30 个字，可重命名。
4. 新对话绑定当前标签的主机（host_id / 标签标题）；正在生成时禁用切换和新建。
5. 锁定保险库时清空面板中的对话（现有锁定流程会重置终端，参照它处理）。
6. 列表为空、加载中、失败都要有状态显示。
7. 更新 dev/mock-ipc.js：内存里放几条示例对话，支持增删改查。

验收：
- 模拟界面：提问 → 关闭面板 → 打开历史能看到并继续；切换“当前主机 / 全部”；重命名、删除正常。
- 单元测试：截断规则、数量上限、同步排除、未解锁时拒绝。
- 直接读 loki.db 搜不到对话里的明文（参照 STATUS 里的密文验证方法）。
- 通用约束第 7 条全部通过；STATUS 新增一节。
```

### 第 1 项：AI 附件（附加即上传到服务器暂存目录）

```
先读 docs/plans/2026-10-29-next-features.md 的「通用约束」，然后完成下面的任务。
本项和“AI 历史会话”都改 src/features/ai/AiPanel.tsx，不要两项同时做。

目标：用户在 AI 对话里附加本机文件（证书、私钥、脚本、配置），文件立即通过当前 SSH 连接
上传到服务器的暂存目录；AI 拿到远端路径后，用现有的 run_command 等工具完成安装、替换、
重载、执行。典型场景：“把这个证书换到 nginx 上”“把这个脚本跑一下”。

设计要点（按这个做，不要改成“AI 调用上传工具”）：
- 上传是用户的动作：附加文件 = 同意把它传到这台服务器，不再单独确认。
- 不新增 AI 工具。后续改动服务器的操作都走现有的 run_command / write_file，
  继续受权限模式和确认流程约束（src-tauri/src/ai/tools.rs 的 may_run_unattended）。
- 文件内容由 Rust 读取，不经过 WebView，也不回传给前端。
- 私钥内容永远不发给模型（见下面“私钥保护”）。
- 只支持 SSH 标签。Telnet、串口、本地终端的标签里回形针置灰，提示“仅 SSH 连接支持附件”。

一、后端（Rust）
1. 命令 ai_attach(session_id, paths: string[]) → AttachmentView[]：
   - 路径来自前端的原生打开对话框（@tauri-apps/plugin-dialog 的 open，multiple）或拖放事件。
     这里允许前端传路径，因为只读用户选中的文件，且内容不会回传给前端。
   - 每个文件：读取、算 sha256、判定 kind（合法 UTF-8 且不含 NUL 为 text，否则 binary）、
     判定 secret（内容含 "PRIVATE KEY-----"，或扩展名为 .key / .pem 且含私钥块 / .pfx / .p12 / .jks）。
   - 上限：单个文件 20 MB，一次最多 10 个，超出直接报错说明原因。
   - 上传：复用 commands.rs 的 sftp_for 拿到 SFTP 会话。暂存目录为
     ~/.loki/uploads/<YYYYMMDD-HHMMSS>-<4位随机hex>/，逐级创建并把 ~/.loki、uploads、
     批次目录权限设为 700；文件先写 <名>.part 再改名，权限 600。
   - 校验：用 ssh.rs 的 exec_with_timeout 在服务器上跑
     `sha256sum <path> 2>/dev/null || shasum -a 256 <path>`（路径要做 shell 转义），
     和本地哈希比对；不一致或命令不存在时如实标记 verified: false，不要假装成功。
   - 通过 transfer-event 或新事件推上传进度（大文件时前端要能显示进度）。
   - 返回 AttachmentView（camelCase）：id、name、size、sha256、kind、secret、remotePath、
     verified、shareable（kind == text && !secret && size <= 64 KB）。不含内容、不含本机路径。
2. AppState 增加附件表 attachment_id → { session_id, name, size, sha256, kind, secret,
   remote_path, content: Option<String>（仅 shareable 时缓存）}。锁定保险库
   （commands.rs 的 close_everything）时清空。
3. ai_detach(id)：从表中移除；如果这个附件还没随消息发出过，同时删除服务器上的文件。
4. ai_attachments_cleanup(session_id)：删除本次运行中为该会话创建的所有暂存批次目录，
   返回删除的文件数。
5. ai_send 增加参数 attachments: [{ id, shareContent: bool }]。由 Rust 在本轮用户消息末尾
   追加附件说明（前端拿不到内容，所以必须在 Rust 里拼）：
     附件（已上传到服务器）：
     #1 example.com.crt，2.1 KB，sha256 3fa9c1e2b7d0…，路径 ~/.loki/uploads/…/example.com.crt
     #2 example.com.key，1.7 KB，含私钥：内容不提供，只能用路径操作
   shareable 且 shareContent 为真的文件，再附上内容代码块。
   以后的轮次不重复附内容；AI 需要时可以用 read_file 读远端文件。
6. 私钥保护（这是本项最重要的部分，必须有单元测试）：
   - 写一个纯函数 redact_private_keys(text) -> String：把
     -----BEGIN … PRIVATE KEY----- 到对应 END 行之间的内容替换成“[私钥内容已隐藏]”，
     覆盖 RSA / EC / OPENSSH / ENCRYPTED / 无类型 PRIVATE KEY，多个块、没有 END 的截断块
     （从 BEGIN 起全部隐藏）、CRLF 换行。
   - 所有发给模型的工具结果（read_file、run_command、read_terminal、grep 等）和
     terminal_tail 都先过这个函数。这样即使 AI 执行了 `cat xxx.key`，私钥也不会到达模型。
   - read_file 读取的路径如果等于某个 secret 附件的 remotePath，直接返回“这是私钥文件，
     内容不提供”。
   - 构造附件说明时 secret 文件绝不读入内容。
7. 系统提示词（src-tauri/src/ai/mod.rs 的 SYSTEM_PROMPT）补充：
   - 用户附加的文件已在服务器暂存目录，用路径操作，不要尝试读取标记为私钥的文件内容。
   - 替换证书、配置前先备份原文件（加日期后缀）；先校验（证书与私钥是否配对、域名和有效期，
     nginx -t / apachectl configtest 等）再重载服务；完成后告诉用户结果，并提醒可清理暂存文件。
   - 执行脚本前先说明脚本会做什么。

二、前端（AiPanel.tsx）
1. 输入框左侧加回形针按钮（多选）。也支持把文件拖到助手面板上：参照
   src/features/sftp/SftpPanel.tsx 里 getCurrentWebview().onDragDropEvent 的写法，
   判断落点是否在助手面板内，不要和文件面板抢。
2. 输入框上方显示附件卡片：上传中显示进度，完成后显示文件名、大小、远端路径（可复制）；
   secret 文件显示锁图标和“私钥内容不会发给 AI”；shareable 文件有“让 AI 看到内容”开关
   （默认开）；verified 为 false 时显示警告；可移除（调用 ai_detach）。
3. 用户消息气泡里显示本次附带的文件（只显示名称、大小、远端路径）。
4. 面板里当前会话有暂存文件时，显示一行“服务器上有 N 个暂存文件 · 清理”，点击确认后调用
   ai_attachments_cleanup。
5. 历史会话（如果已实现）只保存附件元数据：name、size、sha256、remotePath、secret。
6. 更新 dev/mock-ipc.js：ai_attach 返回示例附件（一个证书、一个私钥、一个脚本），
   ai_send 的流式脚本演示一次证书更新流程（备份 → 替换 → nginx -t → reload）。

三、SFTP 面板「发给 AI」
在 SftpPanel.tsx 文件右键菜单加一项「发给 AI」：打开 AI 面板，把该文件的远端路径
（用反引号包起来）插入 AI 输入框光标处。用一个小的 Zustand store 或现有 store 传递，
不要用全局 window 事件。

验收：
- 单元测试：secret 判定；redact_private_keys 的各种情况；附件说明里绝不出现私钥内容；
  大小和数量上限；暂存路径生成和 shell 转义。
- 用本机测试 sshd（docs/STATUS.md 里有配置方法）实测：
  · 附加一个脚本 → 服务器上出现在暂存目录、权限 600、哈希一致 → 让 AI 执行，结果正确。
  · 附加一对自签证书和私钥 → 让 AI 把它们装到一个测试目录并用 openssl 校验配对；
    在 provider 发请求处加 debug 断言，或者用本地假模型端点抓请求体，证明私钥内容从未发出，
    包括 AI 执行 `cat` 私钥之后。
  · 移除未发送的附件，服务器上的文件被删除；「清理」删除整个暂存批次。
- Telnet / 串口 / 本地标签里回形针不可用且有提示。
- 通用约束第 7 条全部通过；STATUS 新增一节，写清楚安全设计和未验证项。
```

---

## 五、完成后的检查清单（给用户）

- [ ] 每项一个提交，main 上 CI（macOS + Windows）全绿
- [ ] docs/STATUS.md 每项都有一节，区分“已验证 / 未验证”
- [ ] 模拟界面能演示四项功能
- [ ] 第 1 项：抓包或断言证明私钥内容没有发给模型（包括 AI 执行 `cat` 私钥之后）；暂存文件权限 600、哈希一致，「清理」能删掉
- [ ] 第 2 项：loki.db 里搜不到对话明文；对话不在同步推送里
- [ ] 第 3 项：录制文件不含控制字符；锁定时自动停止
- [ ] 准备发布时再让 Claude 或 DeepSeek 统一改版本号、写版本说明、打标签
