/**
 * The one place the UI talks to Rust. Every function here maps to a command in
 * `src-tauri/src/commands.rs`; nothing returns a secret.
 */
import { invoke } from "@tauri-apps/api/core";

export interface VaultStatus {
  exists: boolean;
  unlocked: boolean;
  kdf: { m_kib: number; t: number; p: number } | null;
}

export type Protocol = "ssh" | "telnet" | "serial";

export interface HostView {
  id: string;
  name: string;
  protocol: Protocol;
  address: string;
  port: number;
  username: string;
  tags: string[];
  baud: number | null;
  /** Snake case: HostView is serialized without a rename rule. */
  data_bits: number | null;
  parity: string | null;
  stop_bits: number | null;
  /** Whether a password is stored — never the password itself. */
  has_password: boolean;
  /** Whether a private key is stored — never the key itself. */
  has_key: boolean;
  key_id: string | null;
  jump_host_id: string | null;
  use_agent: boolean;
  startup_command: string | null;
}

export interface HostInput {
  id?: string;
  name: string;
  protocol: Protocol;
  address: string;
  port: number;
  username: string;
  tags: string[];
  baud?: number | null;
  dataBits?: number | null;
  parity?: string | null;
  stopBits?: number | null;
  password?: string;
  privateKey?: string;
  clearPassword?: boolean;
  clearKey?: boolean;
  keyId?: string | null;
  jumpHostId?: string | null;
  useAgent?: boolean;
  startupCommand?: string | null;
}

export interface SessionView {
  id: string;
  title: string;
  status: string;
}

export interface AppInfo {
  name: string;
  version: string;
}

export interface CreateVaultResult {
  status: VaultStatus;
  recoveryCode: string | null;
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),

  vaultStatus: () => invoke<VaultStatus>("vault_status"),
  vaultCreate: (password: string) => invoke<CreateVaultResult>("vault_create", { password }),
  vaultRecover: (code: string) => invoke<VaultStatus>("vault_recover", { code }),
  /** Writes the just-created recovery code to a file the user picked. */
  recoveryCodeExport: (path: string, code: string) =>
    invoke<void>("recovery_code_export", { path, code }),
  vaultChangePassword: (newPassword: string) =>
    invoke<VaultStatus>("vault_change_password", { newPassword }),
  vaultUnlock: (password: string) => invoke<VaultStatus>("vault_unlock", { password }),
  vaultLock: () => invoke<VaultStatus>("vault_lock"),

  hostsList: () => invoke<HostView[]>("hosts_list"),
  hostSave: (host: HostInput) => invoke<string>("host_save", { host }),
  hostDelete: (id: string) => invoke<void>("host_delete", { id }),
  categoriesList: () => invoke<CategoryView[]>("categories_list"),
  categorySave: (input: { id?: string; name: string; color?: string | null }) =>
    invoke<string>("category_save", { input }),
  categoryDelete: (id: string) => invoke<void>("category_delete", { id }),
  categoriesReorder: (ids: string[]) => invoke<void>("categories_reorder", { ids }),
  hostDuplicate: (id: string) => invoke<string>("host_duplicate", { id }),

  sessionsList: () => invoke<SessionView[]>("sessions_list"),
  sessionClose: (id: string) => invoke<void>("session_close", { id }),
};

/** Normalizes the string errors Rust returns into something displayable. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Stable per-tag colour, so "prod" is always the same hue. */
/**
 * A host category. Hosts list theirs by name in `HostView.tags`, so a host can
 * be in several; this record adds the order and an optional colour.
 */
export interface CategoryView {
  id: string;
  name: string;
  color: string | null;
  order: number;
  count: number;
}

/** Colours offered when picking one for a category. */
export const CATEGORY_COLORS = [
  "#3fb68b",
  "#4c9aff",
  "#a78bfa",
  "#f472b6",
  "#f59e0b",
  "#ef4444",
  "#14b8a6",
  "#94a3b8",
];

/** A category's colour: the one picked, else derived from its name. */
export function categoryColor(name: string, categories: CategoryView[]): string {
  const c = categories.find((x) => x.name.toLowerCase() === name.toLowerCase());
  return c?.color || tagColor(name);
}

/** A host's categories in the user's category order. */
export function sortedCategories(tags: string[], categories: CategoryView[]): string[] {
  const rank = (t: string) => {
    const i = categories.findIndex((c) => c.name.toLowerCase() === t.toLowerCase());
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  return [...tags].sort((a, b) => rank(a) - rank(b));
}

/** Token counts for Chinese readers: `1.2 万`, `104.9 万`; plain below ten thousand. */
export function formatTokens(n: number): string {
  if (n >= 100_000_000) return `${+(n / 100_000_000).toFixed(1)} 亿`;
  return n >= 10_000 ? `${+(n / 10_000).toFixed(1)} 万` : String(n);
}

export function tagColor(tag: string): string {
  let h = 0;
  for (let i = 0; i < tag.length; i++) h = (h * 31 + tag.charCodeAt(i)) % 360;
  return `hsl(${h}, 62%, 58%)`;
}

// ---- Terminal / sessions -------------------------------------------------

export interface ConnectResult {
  kind: "connected" | "hostKey" | "needPassword" | "needUsername";
  session: SessionView | null;
  fingerprint: string | null;
  keyType: string | null;
  host: string | null;
  port: number | null;
  /** Set when the host was known under a different key: it *changed*. */
  previous: string | null;
  username: string | null;
}

export type TermEvent =
  | { type: "data"; sessionId: string; data: string }
  | { type: "exit"; sessionId: string; code: number }
  | { type: "closed"; sessionId: string };

export interface QuickTarget {
  address: string;
  port: number;
  username: string;
}

export interface OpenInput {
  hostId?: string | null;
  target?: QuickTarget | null;
  accept?: string | null;
  password?: string | null;
  passwordHost?: string | null;
  rememberPassword?: boolean;
  /** Login name typed at the prompt, for a host saved without one. */
  username?: string | null;
  rememberUsername?: boolean;
  cols?: number;
  rows?: number;
}

export interface ServerStats {
  cpuPercent: number | null;
  cpus: number | null;
  memTotal: number | null;
  memUsed: number | null;
  diskTotal: number | null;
  diskUsed: number | null;
  load: [number, number, number] | null;
  uptimeSecs: number | null;
  netRxRate: number | null;
  netTxRate: number | null;
}

export const sessionApi = {
  open: (input: OpenInput) => invoke<ConnectResult>("session_open", { input }),
  write: (sessionId: string, data: string) => invoke<void>("term_write", { sessionId, data }),
  close: (id: string) => invoke<void>("session_close", { id }),
  list: () => invoke<SessionView[]>("sessions_list"),
  stats: (sessionId: string) => invoke<ServerStats>("session_stats", { sessionId }),
  local: (cols: number, rows: number) => invoke<DirectInfo>("local_open", { cols, rows }),
};

export interface KnownHostView {
  id: string;
  host: string;
  port: number;
  keyType: string;
  fingerprint: string;
  addedAt: number;
}

export const knownHostsApi = {
  list: () => invoke<KnownHostView[]>("known_hosts_list"),
  remove: (id: string) => invoke<void>("known_host_delete", { id }),
};

export interface SshConfigCandidate {
  alias: string;
  hostname: string;
  user: string;
  port: number;
  identityFile: string | null;
  proxyJump: string | null;
  exists: boolean;
}

export interface ImportReport {
  hosts: number;
  keys: number;
  notes: string[];
}

export const importApi = {
  scan: () => invoke<SshConfigCandidate[]>("ssh_config_scan"),
  run: (aliases: string[]) => invoke<ImportReport>("ssh_config_import", { aliases }),
};

export const termApi = {
  resize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>("term_resize", { sessionId, cols, rows }),
};

export const termBacklog = (sessionId: string) => invoke<string>("term_backlog", { sessionId });

// ---- Keys ----------------------------------------------------------------

export interface KeyView {
  id: string;
  name: string;
  kind: string;
  fingerprint: string;
  publicKey: string;
  createdAt: number;
}

export interface KeyGenInput {
  name: string;
  kind: string;
  comment: string;
}

export const keyApi = {
  list: () => invoke<KeyView[]>("keys_list"),
  generate: (input: KeyGenInput) => invoke<KeyView>("key_generate", { input }),
  import: (input: { name: string; privateKey?: string; path?: string; passphrase?: string }) =>
    invoke<KeyView>("key_import", { input }),
  rename: (id: string, name: string) => invoke<void>("key_rename", { id, name }),
  remove: (id: string) => invoke<void>("key_delete", { id }),
};

// ---- SFTP ----------------------------------------------------------------

export interface FileEntry {
  name: string;
  path: string;
  kind: "file" | "dir" | "link";
  size: number;
  mode: string | null;
  modified: number | null;
}

export interface DirListing {
  path: string;
  home: string;
  entries: FileEntry[];
}

export const sftpApi = {
  list: (sessionId: string, path: string) =>
    invoke<DirListing>("sftp_list", { sessionId, path }),
  read: (sessionId: string, path: string) => invoke<string>("sftp_read", { sessionId, path }),
  write: (sessionId: string, path: string, content: string) =>
    invoke<void>("sftp_write", { sessionId, path, content }),
  mkdir: (sessionId: string, path: string) => invoke<void>("sftp_mkdir", { sessionId, path }),
  remove: (sessionId: string, path: string, isDir: boolean) =>
    invoke<void>("sftp_delete", { sessionId, path, isDir }),
  rename: (sessionId: string, from: string, to: string) =>
    invoke<void>("sftp_rename", { sessionId, from, to }),
  download: (sessionId: string, remote: string, local: string) =>
    invoke<string>("sftp_download", { sessionId, remote, local }),
  downloadToDownloads: (sessionId: string, remote: string) =>
    invoke<string>("sftp_download_to_downloads", { sessionId, remote }),
  upload: (sessionId: string, locals: string[], remoteDir: string) =>
    invoke<string[]>("sftp_upload", { sessionId, locals, remoteDir }),
  cancel: (id: string) => invoke<void>("transfer_cancel", { id }),
};

export interface TransferEvent {
  id: string;
  sessionId: string;
  direction: "upload" | "download";
  name: string;
  bytes: number;
  total: number;
  state: "running" | "done" | "failed" | "cancelled";
  error: string | null;
}

// ---- Forwards ------------------------------------------------------------

export interface ForwardSpec {
  kind: "local" | "socks";
  localPort: number;
  remoteHost?: string | null;
  remotePort?: number | null;
}

export interface ForwardView {
  id: string;
  sessionId: string;
  kind: string;
  localPort: number;
  remote: string | null;
}

export const forwardApi = {
  start: (sessionId: string, spec: ForwardSpec) =>
    invoke<ForwardView>("forward_start", { sessionId, spec }),
  list: () => invoke<ForwardView[]>("forward_list"),
  stop: (id: string) => invoke<void>("forward_stop", { id }),
};

// ---- AI assistant --------------------------------------------------------

export type PermissionMode = "manual" | "auto" | "bypass";

export type ProviderProtocol = "chat_completions" | "responses" | "anthropic";

export interface ProviderView {
  id: string;
  name: string;
  protocol: ProviderProtocol;
  baseUrl: string;
  /** Every model the user added, in their order. */
  models: string[];
  hasKey: boolean;
  /** Context size the user set for every model; null means automatic. */
  contextLimit: number | null;
  /** Context sizes found automatically, by model id. */
  contextLimits: Record<string, number>;
}

export interface ProviderInput {
  id?: string;
  name: string;
  protocol: string;
  baseUrl: string;
  models: string[];
  apiKey?: string;
  contextLimit?: number | null;
}

export interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  toolCalls?: { id: string; name: string; arguments: unknown }[];
  toolCallId?: string;
}

export type AiEvent =
  | { type: "text"; text: string }
  | { type: "toolCall"; id: string; name: string; arguments: unknown; auto: boolean }
  | { type: "toolResult"; id: string; ok: boolean; content: string }
  | { type: "context"; droppedRounds: number; shortenedOutputs: number; tokens: number; limit: number }
  | { type: "limitLearned"; tokens: number; exact: boolean }
  | { type: "done"; steps: number }
  | { type: "error"; message: string }
  | { type: "stopped" };

/** An event as emitted: tagged with the turn it belongs to. */
export type AiEventPayload = AiEvent & { turnId?: string };

export const aiApi = {
  providers: () => invoke<ProviderView[]>("ai_providers_list"),
  saveProvider: (input: ProviderInput) => invoke<ProviderView>("ai_provider_save", { input }),
  deleteProvider: (id: string) => invoke<void>("ai_provider_delete", { id }),
  /** Fetches the provider's model list without saving. */
  models: (input: ProviderInput) => invoke<string[]>("ai_provider_models", { input }),
  /** Sends a minimal request to prove URL + key work. */
  test: (input: ProviderInput) => invoke<string>("ai_provider_test", { input }),
  send: (
    providerId: string,
    model: string | null,
    sessionId: string | null,
    mode: PermissionMode,
    history: ChatMessage[],
    attachments: { id: string; shareContent: boolean }[] = [],
    turnId: string | null = null,
  ) => invoke<void>("ai_send", { providerId, model, sessionId, mode, history, attachments, turnId }),
  approve: (callId: string, approved: boolean) =>
    invoke<void>("ai_approve", { callId, approved }),
  /** Stops the running turn (the one named, or whichever runs). */
  cancel: (turnId: string | null = null) => invoke<void>("ai_cancel", { turnId }),
};

// ---- Sync ----------------------------------------------------------------

export interface SyncStatus {
  url: string;
  cursor: number;
}

export interface SyncReport {
  pushed: number;
  pulled: number;
  rejected: number;
  cursor: number;
}

export const syncApi = {
  connect: (url: string, token: string) => invoke<string>("sync_connect", { input: { url, token } }),
  status: () => invoke<SyncStatus | null>("sync_status"),
  run: () => invoke<SyncReport>("sync_run"),
  disconnect: () => invoke<void>("sync_disconnect"),
};

// ---- Telnet & serial -----------------------------------------------------

export interface SerialPortInfo {
  path: string;
  name: string;
  vendor: string | null;
  product: string | null;
}

export interface DirectInfo {
  id: string;
  status: string;
  kind: "telnet" | "serial" | "local";
  target: string;
}

export const directApi = {
  serialPorts: () => invoke<SerialPortInfo[]>("serial_ports"),
  telnetOpen: (input: { host: string; port: number; cols?: number; rows?: number }) =>
    invoke<DirectInfo>("telnet_open", { input }),
  serialOpen: (input: {
    path: string;
    baud: number;
    dataBits?: number;
    parity?: string;
    stopBits?: number;
  }) => invoke<DirectInfo>("serial_open", { input }),
  write: (sessionId: string, data: string) =>
    invoke<void>("direct_write", { sessionId, data }),
  resize: (sessionId: string, cols: number, rows: number) =>
    invoke<void>("direct_resize", { sessionId, cols, rows }),
  close: (sessionId: string) => invoke<void>("direct_close", { sessionId }),
};

/** `1.2 GB`-style sizes. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes)) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/** Parses `user@host:port`, `host:port` or `host`; `null` when it is not one. */
export function parseTarget(text: string, fallbackUser: string): QuickTarget | null {
  const t = text.trim().replace(/^ssh\s+/, "");
  const m = /^(?:([^@\s]+)@)?(\[[^\]]+\]|[^:\s@]+)(?::(\d{1,5}))?$/.exec(t);
  if (!m) return null;
  const address = m[2].replace(/^\[|\]$/g, "");
  const port = m[3] ? Number(m[3]) : 22;
  if (!address || port < 1 || port > 65535) return null;
  return { address, port, username: m[1] ?? fallbackUser };
}

// ---- Updates -------------------------------------------------------------

export interface UpdateInfo {
  currentVersion: string;
  version: string;
  notes: string | null;
  date: string | null;
}

export const updateApi = {
  /** `null` means already current — an error means the check itself failed. */
  check: () => invoke<UpdateInfo | null>("update_check"),
  install: () => invoke<void>("update_install"),
};

// ---- Terminal export & session recording ---------------------------------

export interface RecordInfo {
  path: string;
  startedAt: string;
  bytes: number;
}

export interface RecordStopped {
  path: string;
  bytes: number;
}

export const recordApi = {
  /** Writes `text` to a file the user picks; `null` when they cancel. */
  exportText: (defaultName: string, text: string) =>
    invoke<string | null>("terminal_export", { defaultName, text }),
  start: (sessionId: string, plain: boolean) =>
    invoke<RecordInfo | null>("session_record_start", { sessionId, plain }),
  stop: (sessionId: string) =>
    invoke<RecordStopped | null>("session_record_stop", { sessionId }),
  status: (sessionId: string) => invoke<RecordInfo | null>("session_record_status", { sessionId }),
};

// ---- AI conversation history ---------------------------------------------

export interface ConversationSummary {
  id: string;
  title: string;
  hostLabel: string;
  /** Unix seconds. */
  updatedAt: number;
  turnCount: number;
}

export const conversationApi = {
  /** Summaries only; `hostId` narrows to one host. */
  list: (hostId?: string | null) =>
    invoke<ConversationSummary[]>("ai_conversations_list", { hostId: hostId ?? null }),
  get: (id: string) => invoke<unknown | null>("ai_conversation_get", { id }),
  save: (conversation: unknown) => invoke<void>("ai_conversation_save", { conversation }),
  rename: (id: string, title: string) =>
    invoke<void>("ai_conversation_rename", { id, title }),
  remove: (id: string) => invoke<void>("ai_conversation_delete", { id }),
};

// ---- AI attachments ------------------------------------------------------

/** What the panel is told about an uploaded file: no content, no local path. */
export interface AttachmentView {
  id: string;
  name: string;
  size: number;
  sha256: string;
  kind: "text" | "binary";
  secret: boolean;
  remotePath: string;
  verified: boolean;
  /** Text, not secret, and small enough to show the model verbatim. */
  shareable: boolean;
}

export const attachmentApi = {
  attach: (sessionId: string, paths: string[]) =>
    invoke<AttachmentView[]>("ai_attach", { sessionId, paths }),
  detach: (id: string) => invoke<void>("ai_detach", { id }),
  cleanup: (sessionId: string) => invoke<number>("ai_attachments_cleanup", { sessionId }),
};
