// In-memory stand-in for the Rust side, implementing just enough of
// window.__TAURI_INTERNALS__ for every screen to render. Development only.
(() => {
  const callbacks = new Map();
  const listeners = new Map(); // event -> [callbackId]
  let seq = 1;
  const b64 = (s) => btoa(unescape(encodeURIComponent(s)));
  const emit = (event, payload) =>
    (listeners.get(event) || []).forEach((id) => callbacks.get(id)?.({ event, id, payload }));

  // `?fresh` starts from an empty install, to show onboarding and the
  // recovery code step.
  const fresh = location.search.includes("fresh");
  const state = {
    exists: !fresh,
    unlocked: false,
    hosts: [
      host("h1", "prod-api-tokyo", "10.0.12.31", "deploy", ["prod", "k8s"], { has_key: true }),
      host("h2", "staging-web", "staging.example.com", "ubuntu", ["staging"], { has_password: true }),
      host("h3", "db-primary", "10.0.40.2", "postgres", ["prod", "db"], { jump_host_id: "h5", key_id: "k1" }),
      host("h4", "core-switch", "/dev/tty.usbserial-1420", "", ["network"], { protocol: "serial", baud: 9600 }),
      host("h5", "bastion", "bastion.example.com", "ops", ["prod"], { has_key: true }),
      host("h6", "lab-router", "192.168.1.1", "", ["lab"], { protocol: "telnet", port: 23 }),
    ],
    keys: [
      { id: "k1", name: "loki-2026-10-29", kind: "ed25519", fingerprint: "SHA256:QrhJScFAmN1bMZe5Yz7V8BGKQ00UtF5x8Iiual2NXfU", publicKey: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKq0x7n0mock loki", createdAt: 0 },
      { id: "k2", name: "id_rsa (work)", kind: "rsa", fingerprint: "SHA256:9fWm2lq7Yk3cJxv0p8bWm2lq7Yk3cJxv0p8bWm2lq7Y", publicKey: "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQmock work", createdAt: 0 },
    ],
    known: [
      { id: "n1", host: "10.0.12.31", port: 22, keyType: "ssh-ed25519", fingerprint: "SHA256:pXOdQFW/VQL7B9zkRE530IGzuGg1zHiwub1HeZAg4kE", addedAt: 0 },
    ],
    forwards: [],
    opened: 0,
  };

  function host(id, name, address, username, tags, extra = {}) {
    return {
      id, name, protocol: "ssh", address, port: 22, username, tags,
      baud: null, data_bits: null, parity: null, stop_bits: null,
      has_password: false, has_key: false, key_id: null, jump_host_id: null,
      use_agent: true, startup_command: null, ...extra,
    };
  }

  const banner = (title) =>
    `Welcome to Ubuntu 24.04.1 LTS (GNU/Linux 6.8.0-45-generic x86_64)\r\n\r\n` +
    `  System load:  0.08              Processes:             142\r\n` +
    `  Usage of /:   41.2% of 77.35GB  Users logged in:       1\r\n` +
    `  Memory usage: 23%               IPv4 address for eth0: 10.0.12.31\r\n\r\n` +
    `Last login: Wed Oct 29 09:12:44 2026 from 10.0.0.8\r\n` +
    `\x1b[1;32mdeploy@${title}\x1b[0m:\x1b[1;34m~\x1b[0m$ `;

  const sessions = new Map();

  const handlers = {
    // The listener id is the callback id, so unregisterListener can find it.
    "plugin:event|listen": ({ event, handler }) => {
      listeners.set(event, [...(listeners.get(event) || []), handler]);
      return handler;
    },
    "plugin:event|unlisten": () => null,
    "plugin:webview|get_all_webviews": () => [],
    app_info: () => ({ name: "loki", version: "0.7.0" }),
    vault_status: () => ({ exists: state.exists, unlocked: state.unlocked, kdf: null }),
    vault_unlock: () => ((state.unlocked = true), handlers.vault_status()),
    vault_create: () => {
      state.exists = true;
      state.unlocked = true;
      return { status: handlers.vault_status(), recoveryCode: "NHG8 980J ADBD GH83 F8M4 M2BG R8CE MWRF" };
    },
    recovery_code_export: () => null,
    // A turn streamed token by token, with tool calls in between, the way the
    // engine emits it. The reply mirrors a real ops answer: headings, a
    // numbered list, inline code, a table and a code block.
    ai_send: () => {
      const say = (text) => (text.match(/[\s\S]{1,4}/gu) ?? []).map((t) => ({ type: "text", text: t }));
      const tool = (id, command, content) => [
        { type: "toolCall", id, name: "run_command", arguments: { command }, auto: true },
        { type: "toolResult", id, ok: true, content },
      ];
      const script = [
        ...say("我先收集一下这台机器的基本信息。"),
        ...tool("c1", 'uname -a; echo "=== hostname ==="; hostname', "Linux magpie 5.15.167 #0 SMP aarch64 GNU/Linux\n=== hostname ===\nmagpie"),
        ...tool("c2", 'free -h; echo "=== cpu ==="; grep -c ^processor /proc/cpuinfo', "Mem: 1.9G total, 612M used\n=== cpu ===\n4"),
        ...tool("c3", "ls /etc/init.d/ 2>/dev/null", "dockerd\nlinkease\nttyd\nopenclash"),
        ...tool("c4", "netstat -tulnp 2>/dev/null | head -50", "tcp 0.0.0.0:7681 LISTEN ttyd\ntcp :::9090 LISTEN clash"),
        ...tool("c5", "ps w 2>/dev/null | head -60", "1 root /sbin/procd"),
        ...tool("c6", "ubus call service list 2>/dev/null | head", "{}"),
        ...tool("c7", 'ip -4 addr show | grep -E "^[0-9]+:|inet "', "inet 192.168.1.1/24"),
        ...say(`## 概况

这是一台 **OpenWrt** 路由器（aarch64，4 核，1.9G 内存），主要跑着 OpenClash、Docker 和 linkease。

### 几个值得注意的点

1. **安全**：\`ttyd\` 监听 \`0.0.0.0:7681\`，是免登录的 Web shell 入口；OpenClash 的 **9090 控制面板**也监听所有地址。建议给 ttyd 加密码，把 9090 限制到 \`127.0.0.1\`。
2. **敏感信息**：\`/etc/config/macc\` 里有明文的 \`refresh_token\`（JWT），虽然权限是 600，但会随备份扩散。
3. **可回收**：Docker 装而不用（约 300M+），\`/root/tsup\` 还留着 140M 安装包。
4. 进程里有几个 \`nc 198.18.11.x 443\` 的探测，是 OpenClash 的节点连通性检测，正常现象。

| 端口 | 服务 | 监听 | 建议 |
|---|---|---|---|
| 7681 | ttyd | 0.0.0.0 | 加密码或只监听内网 |
| 9090 | Clash 面板 | ::: | 改为 127.0.0.1 |

限制 9090 的做法：

\`\`\`sh
uci set openclash.config.cn_port='9090'
uci set openclash.config.dashboard_forward_domain='127.0.0.1'
uci commit openclash && /etc/init.d/openclash restart
\`\`\`

> 改之前建议先备份 \`/etc/config/openclash\`。

需要我帮你收紧 ttyd / Clash 9090 的监听范围，或者清掉 Docker 和 tsup 省空间吗？`),
        { type: "done", steps: 8 },
      ];
      script.forEach((ev, i) => setTimeout(() => emit("ai-event", ev), 200 + i * 18));
      return null;
    },
    vault_lock: () => ((state.unlocked = false), handlers.vault_status()),
    hosts_list: () => state.hosts,
    host_save: ({ host: h }) => {
      const id = h.id || `h${Date.now()}`;
      const prev = state.hosts.find((x) => x.id === id);
      const view = { ...host(id, h.name, h.address, h.username, h.tags), ...prev, ...h, id,
        key_id: h.keyId ?? null, jump_host_id: h.jumpHostId ?? null, use_agent: h.useAgent ?? true };
      state.hosts = prev ? state.hosts.map((x) => (x.id === id ? view : x)) : [...state.hosts, view];
      return id;
    },
    host_delete: ({ id }) => ((state.hosts = state.hosts.filter((h) => h.id !== id)), null),
    host_duplicate: ({ id }) => {
      const h = state.hosts.find((x) => x.id === id);
      const copy = { ...h, id: `h${Date.now()}`, name: `${h.name} 副本` };
      state.hosts.push(copy);
      return copy.id;
    },
    keys_list: () => state.keys,
    known_hosts_list: () => state.known,
    known_host_delete: ({ id }) => ((state.known = state.known.filter((k) => k.id !== id)), null),
    ai_providers_list: () => [
      { id: "p1", name: "Claude", protocol: "anthropic", baseUrl: "https://api.anthropic.com/v1", models: ["claude-sonnet-5-5"], hasKey: true },
    ],
    ai_provider_models: () => ["claude-haiku-5-5", "claude-opus-5-5", "claude-sonnet-5-5"],
    ai_provider_test: () => "连接正常，模型回复：pong",
    ai_provider_save: ({ input }) => ({ ...input, hasKey: true }),
    update_check: () => null,
    forward_list: () => state.forwards,
    sync_status: () => null,
    serial_ports: () => [{ path: "/dev/tty.usbserial-1420", name: "usbserial", vendor: null, product: null }],
    ssh_config_scan: () => [
      { alias: "prod", hostname: "10.0.0.5", user: "deploy", port: 2200, identityFile: "~/.ssh/id_ed25519", proxyJump: "bastion", exists: false },
      { alias: "bastion", hostname: "bastion.example.com", user: "ops", port: 22, identityFile: null, proxyJump: null, exists: true },
    ],
    session_open: ({ input }) => {
      const h = state.hosts.find((x) => x.id === input.hostId);
      const name = h ? h.name : input.target?.address;
      // The first unknown host shows the fingerprint prompt, like the real thing.
      if (h?.id === "h2" && !input.accept) {
        return { kind: "hostKey", fingerprint: "SHA256:7Lr4Gq0d4mVxS0kzR8kq7sJmT3hQpV0bXcYdE2fGh1I", keyType: "ssh-ed25519", host: h.address, port: 22, previous: null };
      }
      if (h?.id === "h3" && !input.password) {
        return { kind: "needPassword", host: h.address, username: h.username };
      }
      const id = `s${++state.opened}`;
      sessions.set(id, name);
      setTimeout(() => emit("term-event", { type: "data", sessionId: id, data: b64(banner(name)) }), 50);
      return { kind: "connected", session: { id, title: name, status: "connected" } };
    },
    local_open: () => {
      const id = `l${++state.opened}`;
      setTimeout(() => emit("term-event", { type: "data", sessionId: id, data: b64("\x1b[1;32muser@mac\x1b[0m ~ % ") }), 50);
      return { id, status: "connected", kind: "local", target: "zsh" };
    },
    term_backlog: () => "",
    term_write: ({ sessionId, data }) => {
      const text = decodeURIComponent(escape(atob(data)));
      emit("term-event", { type: "data", sessionId, data: b64(text === "\r" ? "\r\n$ " : text) });
      return null;
    },
    term_resize: () => null,
    direct_write: (a) => handlers.term_write(a),
    direct_resize: () => null,
    direct_close: () => null,
    session_close: () => null,
    session_stats: () => ({
      cpuPercent: 18 + Math.random() * 10, cpus: 4, memTotal: 8 * 2 ** 30, memUsed: 3.1 * 2 ** 30,
      diskTotal: 80 * 2 ** 30, diskUsed: 33 * 2 ** 30, load: [0.42, 0.37, 0.31], uptimeSecs: 1_234_567,
      netRxRate: 182_000, netTxRate: 24_000,
    }),
    sftp_list: ({ path }) => ({
      path: path || "/home/deploy",
      home: "/home/deploy",
      entries: [
        { name: ".bashrc", path: "/home/deploy/.bashrc", kind: "file", size: 3771, mode: "644", modified: 1761700000 },
        { name: "app", path: "/home/deploy/app", kind: "dir", size: 4096, mode: "755", modified: 1761720000 },
        { name: "logs", path: "/home/deploy/logs", kind: "dir", size: 4096, mode: "755", modified: 1761730000 },
        { name: "current", path: "/home/deploy/current", kind: "link", size: 20, mode: "777", modified: 1761730000 },
        { name: "docker-compose.yml", path: "/home/deploy/docker-compose.yml", kind: "file", size: 1820, mode: "644", modified: 1761710000 },
        { name: "backup-2026-10-28.tar.gz", path: "/home/deploy/backup.tar.gz", kind: "file", size: 734003200, mode: "600", modified: 1761600000 },
      ],
    }),
    sftp_download_to_downloads: () => {
      const id = `t${Date.now()}`;
      let bytes = 0;
      const total = 734003200;
      const tick = () => {
        bytes = Math.min(total, bytes + 90_000_000);
        emit("transfer-event", { id, sessionId: "s1", direction: "download", name: "backup-2026-10-28.tar.gz", bytes, total, state: bytes >= total ? "done" : "running", error: null });
        if (bytes < total) setTimeout(tick, 400);
      };
      tick();
      return id;
    },
  };

  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    plugins: { path: { sep: "/", delimiter: ":" } },
    callbacks,
    transformCallback(cb) {
      const id = seq++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    runCallback(id, data) {
      callbacks.get(id)?.(data);
    },
    convertFileSrc: (p) => p,
    async invoke(cmd, args) {
      const h = handlers[cmd];
      if (!h) {
        console.warn("[mock] unhandled", cmd, args);
        return null;
      }
      return h(args || {});
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(event, id) {
      listeners.set(event, (listeners.get(event) || []).filter((x) => x !== id));
    },
  };
  window.__lokiMock = { state, emit };
})();
