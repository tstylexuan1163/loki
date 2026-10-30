import { useEffect, useRef, useState } from "react";
import { useApp, type HostPrefill } from "@/app/store";
import { usePrefs } from "@/app/prefs";
import { serialLabel, useTerminals, type TermTab } from "@/features/terminal/terminals";
import { TerminalPane, terminalRegistry } from "@/features/terminal/TerminalPane";
import { StatusBar } from "@/features/terminal/StatusBar";
import { QuickConnect } from "@/features/terminal/QuickConnect";
import { SftpPanel } from "@/features/sftp/SftpPanel";
import { AiPanel } from "@/features/ai/AiPanel";
import { useAiComposer } from "@/features/ai/aiComposer";
import { Menu, confirmDialog, toast, useMenu } from "@/components/ui";
import { isAppShortcut, keyLabel, shortcutKey } from "@/app/platform";
import { recordApi, formatBytes } from "@/lib/api";
import { bufferToText, fileStamp, safeStem } from "@/features/terminal/exportText";
import {
  IconAlert,
  IconEye,
  IconEyeOff,
  IconChevronDown,
  IconChevronRight,
  IconCopy,
  IconFolder,
  IconPlus,
  IconRefresh,
  IconSearch,
  IconServer,
  IconShield,
  IconSparkle,
  IconTerminal,
  IconUsb,
  IconX,
  IconGlobe,
  IconEdit,
  IconDownload,
} from "@/components/icons";

type Panel = "files" | "ai" | null;

const KIND_ICON = {
  ssh: <IconServer size={13} />,
  telnet: <IconGlobe size={13} />,
  serial: <IconUsb size={13} />,
  local: <IconTerminal size={13} />,
};

/**
 * The terminal workspace: tab strip, one pane per tab (all kept mounted), the
 * optional side panel, and a status bar.
 *
 * Always mounted by App, even while another page is showing, so switching to
 * the host list and back does not tear down any terminal.
 */
export function TerminalView({ visible }: { visible: boolean }) {
  const tabs = useTerminals((s) => s.tabs);
  const active = useTerminals((s) => s.active);
  const sendInput = useTerminals((s) => s.sendInput);
  const resize = useTerminals((s) => s.resize);
  const closeTab = useTerminals((s) => s.closeTab);
  const closeOthers = useTerminals((s) => s.closeOthers);
  const activate = useTerminals((s) => s.activate);
  const reconnect = useTerminals((s) => s.reconnect);
  const rename = useTerminals((s) => s.rename);
  const setRecording = useTerminals((s) => s.setRecording);
  const openLocal = useTerminals((s) => s.openLocal);
  const panelWidth = usePrefs((s) => s.panelWidth);
  const setPref = usePrefs((s) => s.set);

  const [panel, setPanel] = useState<Panel>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [quick, setQuick] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const tabMenu = useMenu<TermTab>();
  const moreMenu = useMenu<undefined>();

  /** Exports the scrollback (or the selection) of a tab. */
  async function exportTab(tab: TermTab, selection: boolean) {
    const entry = terminalRegistry.get(tab.id);
    if (!entry) {
      toast("这个标签还没有终端内容", "error");
      return;
    }
    const text = selection ? entry.term.getSelection() : bufferToText(entry.term);
    if (!text.trim()) {
      toast(selection ? "没有选中的内容" : "终端还没有输出", "info");
      return;
    }
    const name = `${safeStem(tab.title)}-${fileStamp()}.txt`;
    try {
      const path = await recordApi.exportText(name, text);
      if (path) toast(`已导出到 ${path}`, "success");
    } catch (e) {
      toast(String(e), "error");
    }
  }

  /** Starts recording a session's output to a file. */
  async function startRecording(tab: TermTab, plain: boolean) {
    if (!tab.sessionId || tab.status !== "connected") {
      toast("只有已连接的标签可以录制", "info");
      return;
    }
    const ok = await confirmDialog({
      title: "开始录制会话",
      body: (
        <div className="rec-hint">
          <p>录制会从此刻起把该会话的输出写入文件，超过回滚行数的内容也不会丢。</p>
          <p>
            录交换机的长配置前，先关闭分页：Cisco <code>terminal length 0</code>，华为 / H3C{" "}
            <code>screen-length 0 temporary</code>，Juniper <code>set cli screen-length 0</code>。
          </p>
          <p className="muted">
            {plain
              ? "文件为纯文本，颜色控制符会被去掉。"
              : "保留原始控制字符（含颜色），适合之后回放。"}
          </p>
        </div>
      ),
      confirmLabel: "选择文件并开始",
    });
    if (!ok) return;

    try {
      const info = await recordApi.start(tab.sessionId, plain);
      if (info) {
        setRecording(tab.id, info);
        toast(`录制中：${info.path}`, "success");
      }
    } catch (e) {
      toast(String(e), "error");
    }
  }

  /** Stops recording and reports where the file went. */
  async function stopRecording(tab: TermTab) {
    if (!tab.sessionId) return;
    try {
      const res = await recordApi.stop(tab.sessionId);
      setRecording(tab.id, null);
      if (res) toast(`已保存 ${res.path}（${formatBytes(res.bytes)}）`, "success");
    } catch (e) {
      toast(String(e), "error");
    }
  }

  /** The three actions, shared by the tab menu and the toolbar menu. */
  function sessionActionItems(tab: TermTab) {
    const connected = tab.status === "connected" && !!tab.sessionId;
    const items = [
      {
        label: "导出终端内容…",
        icon: <IconDownload size={14} />,
        onSelect: () => void exportTab(tab, false),
      },
      {
        label: "导出选中内容…",
        icon: <IconDownload size={14} />,
        onSelect: () => void exportTab(tab, true),
      },
      "sep" as const,
    ];
    if (tab.recording) {
      items.push({
        label: "停止录制",
        icon: <IconAlert size={14} />,
        hint: formatBytes(tab.recording.bytes),
        danger: true,
        onSelect: () => void stopRecording(tab),
      } as never);
    } else {
      items.push({
        label: "开始录制…",
        icon: <IconAlert size={14} />,
        disabled: !connected,
        onSelect: () => void startRecording(tab, true),
      } as never);
      items.push({
        label: "开始录制（保留颜色）…",
        icon: <IconAlert size={14} />,
        disabled: !connected,
        onSelect: () => void startRecording(tab, false),
      } as never);
    }
    return items;
  }

  const current = tabs.find((t) => t.id === active) ?? null;
  const isSsh = current?.kind === "ssh" && current.status === "connected";

  // Shortcuts that only make sense while the terminal page is showing.
  useEffect(() => {
    if (!visible) return;
    const onKey = (e: KeyboardEvent) => {
      if (!isAppShortcut(e)) return;
      const key = shortcutKey(e);
      if (key === "f" && current) {
        e.preventDefault();
        setFindOpen(true);
      } else if (key === "w" && current) {
        e.preventDefault();
        void closeTab(current.id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible, current, closeTab]);

  // The files panel needs an SSH session; close it when the tab has none.
  useEffect(() => {
    if (panel === "files" && !isSsh) setPanel(null);
  }, [panel, isSsh]);

  return (
    <div className="workspace" style={{ display: visible ? "flex" : "none" }}>
      <div className="tabstrip">
        <div className="tabstrip__tabs">
          {tabs.map((t) => (
            <div
              key={t.id}
              className={`tab ${t.id === active ? "tab--active" : ""}`}
              onMouseDown={(e) => {
                // Middle click closes, as in every browser.
                if (e.button === 1) {
                  e.preventDefault();
                  void closeTab(t.id);
                }
              }}
              onClick={() => activate(t.id)}
              onDoubleClick={() => setRenaming(t.id)}
              onContextMenu={(e) => tabMenu.open(e, t)}
              title={t.title}
            >
              <span className={`tab__dot tab__dot--${t.status}`} />
              <span className="tab__kind">{KIND_ICON[t.kind]}</span>
              {t.recording && <span className="tab__rec" title="录制中" />}
              {renaming === t.id ? (
                <input
                  className="tab__rename"
                  autoFocus
                  defaultValue={t.title}
                  onClick={(e) => e.stopPropagation()}
                  onBlur={(e) => {
                    rename(t.id, e.target.value);
                    setRenaming(null);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    if (e.key === "Escape") setRenaming(null);
                  }}
                />
              ) : (
                <span className="tab__title">{t.title}</span>
              )}
              <button
                className="tab__close"
                onClick={(e) => {
                  e.stopPropagation();
                  void closeTab(t.id);
                }}
                aria-label="关闭标签"
              >
                <IconX size={12} />
              </button>
            </div>
          ))}
          <button className="tabstrip__new" onClick={() => setQuick(true)} title={`新建连接 (${keyLabel("T")})`}>
            <IconPlus size={15} />
          </button>
        </div>

        <div className="tabstrip__tools">
          <button
            className={`toolbtn ${findOpen ? "toolbtn--on" : ""}`}
            onClick={() => setFindOpen(!findOpen)}
            disabled={!current}
            title={`在终端中查找 (${keyLabel("F")})`}
          >
            <IconSearch size={15} />
          </button>
          <button
            className={`toolbtn ${panel === "files" ? "toolbtn--on" : ""}`}
            onClick={() => setPanel(panel === "files" ? null : "files")}
            disabled={!isSsh}
            title={isSsh ? "文件 (SFTP)" : "文件面板需要 SSH 会话"}
          >
            <IconFolder size={15} />
            <span>文件</span>
          </button>
          <button
            className={`toolbtn ${panel === "ai" ? "toolbtn--on" : ""}`}
            onClick={() => setPanel(panel === "ai" ? null : "ai")}
            disabled={!current}
            title="AI 助手"
          >
            <IconSparkle size={15} />
            <span>助手</span>
          </button>
          <button
            className="toolbtn"
            onClick={(e) => {
              if (current) moreMenu.openBelow(e.currentTarget, undefined);
            }}
            disabled={!current}
            title="导出与录制"
          >
            <IconDownload size={15} />
            <span>更多</span>
          </button>
        </div>
      </div>

      {tabs.length === 0 ? (
        <div className="content content--center">
          <div className="empty">
            <div className="empty__icon">
              <IconTerminal size={28} />
            </div>
            <div className="empty__title">没有打开的会话</div>
            <div className="empty__hint">从主机列表连接一台服务器，或者打开一个本地终端。</div>
            <div className="empty__actions">
              <button className="btn btn--primary" onClick={() => setQuick(true)}>
                <IconPlus size={14} /> 新建连接
              </button>
              <button className="btn" onClick={() => openLocal()}>
                <IconTerminal size={14} /> 本地终端
              </button>
            </div>
          </div>
        </div>
      ) : (
        <div className="workspace__body">
          <div className="workspace__main">
            {findOpen && current && (
              <FindBar tabId={current.id} onClose={() => setFindOpen(false)} />
            )}
            <div className="panes">
              {tabs.map((t) => (
                <TerminalPane
                  key={t.id}
                  tabId={t.id}
                  sessionId={t.sessionId}
                  active={visible && t.id === active}
                  onInput={(data) => sendInput(t.id, data)}
                  onResize={(cols, rows) => resize(t.id, cols, rows)}
                />
              ))}
              {current && current.status !== "connected" && (
                <ConnectOverlay tab={current} onReconnect={() => reconnect(current.id)} />
              )}
            </div>
            {current && <StatusBar tab={current} />}
          </div>

          {panel && current && (
            <>
              <Splitter width={panelWidth} onChange={(w) => setPref("panelWidth", w)} />
              <div className="sidepanel" style={{ width: panelWidth }}>
                {panel === "files" && current.sessionId && current.kind === "ssh" ? (
                  <SftpPanel
                    key={current.sessionId}
                    sessionId={current.sessionId}
                    onSendToAi={(remotePath) => {
                      setPanel("ai");
                      useAiComposer.getState().setPendingInsert("`" + remotePath + "`");
                    }}
                  />
                ) : panel === "ai" ? (
                  <AiPanel
                    sessionId={current.kind === "ssh" ? current.sessionId : null}
                    hostId={current.hostId}
                    hostLabel={current.title}
                    onClose={() => setPanel(null)}
                  />
                ) : null}
              </div>
            </>
          )}
        </div>
      )}

      {quick && <QuickConnect onClose={() => setQuick(false)} />}

      {moreMenu.menu && current && (
        <Menu
          x={moreMenu.menu.x}
          y={moreMenu.menu.y}
          onClose={moreMenu.close}
          items={sessionActionItems(current)}
        />
      )}

      {tabMenu.menu && (
        <Menu
          x={tabMenu.menu.x}
          y={tabMenu.menu.y}
          onClose={tabMenu.close}
          items={[
            {
              label: "重新连接",
              icon: <IconRefresh size={14} />,
              onSelect: () => reconnect(tabMenu.menu!.data.id),
            },
            {
              label: "重命名",
              icon: <IconEdit size={14} />,
              onSelect: () => setRenaming(tabMenu.menu!.data.id),
            },
            "sep",
            ...sessionActionItems(tabMenu.menu.data),
            {
              label: "保存为主机",
              icon: <IconServer size={14} />,
              // Only for connections that are not already saved to the vault.
              disabled: !prefillFromTab(tabMenu.menu.data),
              onSelect: () => {
                const prefill = prefillFromTab(tabMenu.menu!.data);
                if (!prefill) return;
                useApp.setState({ page: { kind: "host-edit", hostId: null, prefill } });
              },
            },
            {
              label: "复制标签",
              icon: <IconCopy size={14} />,
              disabled:
                !tabMenu.menu.data.hostId &&
                !tabMenu.menu.data.target &&
                !tabMenu.menu.data.direct &&
                tabMenu.menu.data.kind !== "local",
              onSelect: () => duplicate(tabMenu.menu!.data),
            },
            "sep",
            {
              label: "关闭其他标签",
              onSelect: () => closeOthers(tabMenu.menu!.data.id),
              disabled: tabs.length < 2,
            },
            {
              label: "关闭",
              danger: true,
              hint: keyLabel("W"),
              icon: <IconX size={14} />,
              onSelect: () => void closeTab(tabMenu.menu!.data.id),
            },
          ]}
        />
      )}
    </div>
  );
}

function duplicate(tab: TermTab) {
  const t = useTerminals.getState();
  if (tab.kind === "local") t.openLocal();
  else if (tab.target) t.openTarget(tab.target);
  else if (tab.direct?.kind === "telnet")
    t.openTelnet({ host: tab.direct.host, port: tab.direct.port });
  else if (tab.direct?.kind === "serial")
    t.openSerial({
      path: tab.direct.path,
      baud: tab.direct.baud,
      dataBits: tab.direct.dataBits,
      parity: tab.direct.parity,
      stopBits: tab.direct.stopBits,
    });
  else if (tab.hostId) {
    const host = useApp.getState().hosts.find((h) => h.id === tab.hostId);
    if (host) t.openHost(host);
  }
}

/**
 * Connection details of an unsaved tab, for the "save as host" action.
 *
 * Returns null for tabs that already point at a saved host, and for local
 * shells, which have nothing to save.
 */
function prefillFromTab(tab: TermTab): HostPrefill | null {
  if (tab.hostId) return null;
  if (tab.target) {
    const { username, address, port } = tab.target;
    return {
      protocol: "ssh",
      name: username ? `${username}@${address}` : address,
      address,
      port,
      username,
    };
  }
  if (tab.direct?.kind === "telnet") {
    const { host, port } = tab.direct;
    return { protocol: "telnet", name: `${host}:${port}`, address: host, port };
  }
  if (tab.direct?.kind === "serial") {
    const d = tab.direct;
    return {
      protocol: "serial",
      name: serialLabel(d.path),
      address: d.path,
      baud: d.baud,
      dataBits: d.dataBits,
      parity: d.parity,
      stopBits: d.stopBits,
    };
  }
  return null;
}

/** Drag handle between the terminal and the side panel. */
function Splitter({ width, onChange }: { width: number; onChange: (w: number) => void }) {
  const start = useRef<{ x: number; w: number } | null>(null);
  return (
    <div
      className="splitter"
      onMouseDown={(e) => {
        e.preventDefault();
        start.current = { x: e.clientX, w: width };
        const move = (ev: MouseEvent) => {
          if (!start.current) return;
          const w = start.current.w - (ev.clientX - start.current.x);
          onChange(Math.round(Math.max(280, Math.min(720, w))));
        };
        const up = () => {
          start.current = null;
          window.removeEventListener("mousemove", move);
          window.removeEventListener("mouseup", up);
          document.body.classList.remove("is-resizing");
        };
        document.body.classList.add("is-resizing");
        window.addEventListener("mousemove", move);
        window.addEventListener("mouseup", up);
      }}
    />
  );
}

function FindBar({ tabId, onClose }: { tabId: string; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [caseSensitive, setCase] = useState(false);
  const [regex, setRegex] = useState(false);
  const [found, setFound] = useState<boolean | null>(null);
  const entry = terminalRegistry.get(tabId);

  const opts = {
    caseSensitive,
    regex,
    decorations: {
      matchBackground: "#2ee6a833",
      activeMatchBackground: "#d9b16999",
      matchOverviewRuler: "#2ee6a8",
      activeMatchColorOverviewRuler: "#d9b169",
    },
  };

  const run = (dir: 1 | -1, text = q) => {
    if (!entry || !text) {
      setFound(null);
      return;
    }
    const ok =
      dir === 1 ? entry.search.findNext(text, opts) : entry.search.findPrevious(text, opts);
    setFound(ok);
  };

  useEffect(
    () => () => {
      entry?.search.clearDecorations();
      entry?.term.focus();
    },
    [entry],
  );

  return (
    <div className="findbar">
      <IconSearch size={14} />
      <input
        className="findbar__input"
        autoFocus
        placeholder="查找"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        value={q}
        onChange={(e) => {
          setQ(e.target.value);
          run(1, e.target.value);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") run(e.shiftKey ? -1 : 1);
          if (e.key === "Escape") onClose();
        }}
      />
      {found === false && q && <span className="findbar__none">无结果</span>}
      <button
        className={`findbar__opt ${caseSensitive ? "findbar__opt--on" : ""}`}
        onClick={() => setCase(!caseSensitive)}
        title="区分大小写"
      >
        Aa
      </button>
      <button
        className={`findbar__opt ${regex ? "findbar__opt--on" : ""}`}
        onClick={() => setRegex(!regex)}
        title="正则表达式"
      >
        .*
      </button>
      <button className="iconbtn" onClick={() => run(-1)} title="上一个 (⇧↵)">
        <IconChevronDown size={14} style={{ transform: "rotate(180deg)" }} />
      </button>
      <button className="iconbtn" onClick={() => run(1)} title="下一个 (↵)">
        <IconChevronDown size={14} />
      </button>
      <button className="iconbtn" onClick={onClose} title="关闭 (Esc)">
        <IconX size={14} />
      </button>
    </div>
  );
}

/** Covers the pane while the tab is connecting, asking, or has failed. */
function ConnectOverlay({ tab, onReconnect }: { tab: TermTab; onReconnect: () => void }) {
  const hosts = useApp((s) => s.hosts);
  const acceptHostKey = useTerminals((s) => s.acceptHostKey);
  const submitPassword = useTerminals((s) => s.submitPassword);
  const submitUsername = useTerminals((s) => s.submitUsername);
  const cancelPrompt = useTerminals((s) => s.cancelPrompt);
  const closeTab = useTerminals((s) => s.closeTab);
  const host = hosts.find((h) => h.id === tab.hostId);
  const openHost = useApp((s) => s.openHost);

  // A closed session keeps its screen readable; only a slim bar appears.
  if (tab.status === "closed") {
    return (
      <div className="pane-banner">
        <span>连接已断开</span>
        <button className="btn btn--sm btn--primary" onClick={onReconnect}>
          <IconRefresh size={13} /> 重新连接
        </button>
        <button className="btn btn--sm btn--ghost" onClick={() => void closeTab(tab.id)}>
          关闭标签
        </button>
      </div>
    );
  }

  const prompt = tab.prompt;
  return (
    <div className="overlay">
      {prompt?.type === "hostKey" ? (
        <div className={`overlay__card ${prompt.previous ? "overlay__card--danger" : ""}`}>
          <div className="overlay__icon">
            {prompt.previous ? <IconAlert size={22} /> : <IconShield size={22} />}
          </div>
          <h3 className="overlay__title">
            {prompt.previous ? "主机密钥已改变！" : "第一次连接这台主机"}
          </h3>
          <p className="overlay__text">
            {prompt.previous ? (
              <>
                <strong>
                  {prompt.host}:{prompt.port}
                </strong>{" "}
                出示的 {prompt.keyType} 密钥与之前保存的不同。可能是服务器重装了系统，
                <strong>也可能有人在中间拦截</strong>。确认原因前不要继续。
              </>
            ) : (
              <>
                请确认{" "}
                <strong>
                  {prompt.host}:{prompt.port}
                </strong>{" "}
                的主机密钥指纹与服务器上的一致。确认后会记住它，以后不再询问。
              </>
            )}
          </p>
          {prompt.previous && (
            <div className="kv">
              <span>之前</span>
              <code className="fingerprint fingerprint--old">{prompt.previous}</code>
            </div>
          )}
          <div className="kv">
            <span>{prompt.previous ? "现在" : prompt.keyType}</span>
            <code className="fingerprint">{prompt.fingerprint}</code>
          </div>
          <div className="overlay__hint">
            服务器上可用 <code>ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub</code> 查看。
          </div>
          <div className="overlay__actions">
            <button className="btn btn--ghost" onClick={() => cancelPrompt(tab.id)}>
              取消
            </button>
            <button
              className={`btn ${prompt.previous ? "btn--danger" : "btn--primary"}`}
              onClick={() => acceptHostKey(tab.id)}
            >
              {prompt.previous ? "我确认，更新指纹并连接" : "指纹一致，继续连接"}
            </button>
          </div>
        </div>
      ) : prompt?.type === "username" ? (
        <UsernamePrompt
          host={prompt.host}
          canRemember={!!tab.hostId}
          onSubmit={(name, remember) => submitUsername(tab.id, name, remember)}
          onCancel={() => cancelPrompt(tab.id)}
        />
      ) : prompt?.type === "password" ? (
        <PasswordPrompt
          key={`${prompt.host}-${prompt.retry}`}
          host={prompt.host}
          username={prompt.username}
          retry={prompt.retry}
          reason={prompt.reason}
          canRemember={!!tab.hostId}
          onSubmit={(pw, remember) => submitPassword(tab.id, pw, remember)}
          onCancel={() => cancelPrompt(tab.id)}
        />
      ) : tab.status === "failed" ? (
        <div className="overlay__card">
          <div className="overlay__icon overlay__icon--danger">
            <IconAlert size={22} />
          </div>
          <h3 className="overlay__title">连接失败</h3>
          <p className="overlay__text overlay__text--mono">{tab.error}</p>
          <div className="overlay__actions">
            {host && (
              <button className="btn btn--ghost" onClick={() => openHost(host.id)}>
                编辑主机
              </button>
            )}
            <button className="btn btn--ghost" onClick={() => void closeTab(tab.id)}>
              关闭
            </button>
            <button className="btn btn--primary" onClick={onReconnect}>
              <IconRefresh size={14} /> 重试
            </button>
          </div>
        </div>
      ) : (
        <div className="overlay__connecting">
          <span className="spinner" />
          <span>
            正在连接 <strong>{tab.title}</strong>
          </span>
          <IconChevronRight size={14} className="muted" />
        </div>
      )}
    </div>
  );
}

/** Asks for the login name of a host saved without one. */
function UsernamePrompt({
  host,
  canRemember,
  onSubmit,
  onCancel,
}: {
  host: string;
  canRemember: boolean;
  onSubmit: (name: string, remember: boolean) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [remember, setRemember] = useState(true);
  return (
    <form
      className="overlay__card"
      onSubmit={(e) => {
        e.preventDefault();
        if (name.trim()) onSubmit(name, remember);
      }}
    >
      <div className="overlay__icon">
        <IconShield size={22} />
      </div>
      <h3 className="overlay__title">输入用户名</h3>
      <p className="overlay__text">
        这台主机没有设置用户名，用什么账号登录 <strong>{host}</strong>？
      </p>
      <input
        className="field__input"
        autoFocus
        autoComplete="off"
        autoCapitalize="off"
        autoCorrect="off"
        spellCheck={false}
        placeholder="例如 root"
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
      {canRemember && (
        <label className="check">
          <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
          登录成功后保存到主机
        </label>
      )}
      <div className="overlay__actions">
        <button type="button" className="btn btn--ghost" onClick={onCancel}>
          取消
        </button>
        <button type="submit" className="btn btn--primary" disabled={!name.trim()}>
          继续
        </button>
      </div>
    </form>
  );
}

function PasswordPrompt({
  host,
  username,
  retry,
  reason,
  canRemember,
  onSubmit,
  onCancel,
}: {
  host: string;
  username: string;
  retry: boolean;
  reason?: string;
  canRemember: boolean;
  onSubmit: (pw: string, remember: boolean) => void;
  onCancel: () => void;
}) {
  const [pw, setPw] = useState("");
  const [remember, setRemember] = useState(false);
  // What was typed is invisible; showing it, and warning about Caps Lock or a
  // Chinese input method, are what end a "the password is right but it says
  // wrong" loop.
  const [reveal, setReveal] = useState(false);
  const [capsLock, setCapsLock] = useState(false);
  const watchCaps = (e: React.KeyboardEvent | React.MouseEvent) =>
    setCapsLock(e.getModifierState("CapsLock"));
  const nonAscii = /[^\x20-\x7e]/.test(pw);
  return (
    <form
      className="overlay__card"
      onSubmit={(e) => {
        e.preventDefault();
        if (pw) onSubmit(pw, remember);
      }}
    >
      <div className="overlay__icon">
        <IconShield size={22} />
      </div>
      <h3 className="overlay__title">输入密码</h3>
      <p className="overlay__text">
        {retry ? (reason ? `登录失败：${reason}。` : "密码不正确，请重试。") : "密钥和 ssh-agent 都没能登录，"}
        <strong>
          {username ? `${username}@` : ""}
          {host}
        </strong>{" "}
        需要密码。
      </p>
      <div className="pwfield">
        <input
          className="field__input pwfield__input"
          type={reveal ? "text" : "password"}
          autoFocus
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          placeholder="密码"
          value={pw}
          onChange={(e) => setPw(e.target.value)}
          onKeyDown={watchCaps}
          onKeyUp={watchCaps}
          onMouseDown={watchCaps}
        />
        <button
          type="button"
          className="pwfield__eye"
          title={reveal ? "隐藏密码" : "显示密码"}
          // Keep the caret in the field so Enter still logs in.
          tabIndex={-1}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => setReveal((v) => !v)}
        >
          {reveal ? <IconEyeOff size={15} /> : <IconEye size={15} />}
        </button>
      </div>
      {capsLock && (
        <div className="pwfield__warn">
          <IconAlert size={12} /> 大写锁定已开启
        </div>
      )}
      {!capsLock && nonAscii && (
        <div className="pwfield__warn">
          <IconAlert size={12} /> 密码里有中文或全角字符，请确认输入法处于英文状态
        </div>
      )}
      {canRemember && (
        <label className="check">
          <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
          记住密码（加密保存在保险库）
        </label>
      )}
      <div className="overlay__actions">
        <button type="button" className="btn btn--ghost" onClick={onCancel}>
          取消
        </button>
        <button type="submit" className="btn btn--primary" disabled={!pw}>
          登录
        </button>
      </div>
    </form>
  );
}
