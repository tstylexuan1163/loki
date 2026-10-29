import { useEffect, useMemo, useState } from "react";
import { useApp } from "@/app/store";
import { usePrefs, type QuickProtocol } from "@/app/prefs";
import { Modal, Segmented } from "@/components/ui";
import { IconClock, IconGlobe, IconRefresh, IconServer, IconTerminal, IconUsb } from "@/components/icons";
import { directApi, parseTarget, type SerialPortInfo } from "@/lib/api";
import { useTerminals } from "@/features/terminal/terminals";

const BAUD_RATES = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400];
const TELNET_SCHEME = "telnet://";

const PROTOCOLS: { value: QuickProtocol; label: string }[] = [
  { value: "ssh", label: "SSH" },
  { value: "telnet", label: "Telnet" },
  { value: "serial", label: "串口" },
  { value: "local", label: "本地" },
];

/** `host`, `host:port`, or `[v6]:port` → host plus port (23 by default). */
function parseTelnet(text: string): { host: string; port: number } | null {
  const t = text.trim();
  if (!t) return null;
  const bracketed = /^\[([^\]]+)\](?::(\d+))?$/.exec(t);
  if (bracketed) return { host: bracketed[1], port: bracketed[2] ? Number(bracketed[2]) : 23 };
  // Colons inside a bare IPv6 address are not separators; require the tail to
  // be numeric before splitting.
  const m = /^([^:]+):(\d+)$/.exec(t);
  if (m) return { host: m[1], port: Number(m[2]) };
  return { host: t, port: 23 };
}

/**
 * Quick connect: type a target, or pick a saved host, like a command palette.
 *
 * The protocol switch at the top decides what the input means. SSH keeps the
 * original behaviour exactly; telnet takes `host[:port]`; serial is a small form
 * because a device path is picked, not typed from memory.
 */
export function QuickConnect({ onClose }: { onClose: () => void }) {
  const hosts = useApp((s) => s.hosts);
  const recent = usePrefs((s) => s.recentTargets);
  const addRecent = usePrefs((s) => s.addRecent);
  const savedProtocol = usePrefs((s) => s.quickProtocol);
  const setPref = usePrefs((s) => s.set);

  const openHost = useTerminals((s) => s.openHost);
  const openTarget = useTerminals((s) => s.openTarget);
  const openTelnet = useTerminals((s) => s.openTelnet);
  const openSerial = useTerminals((s) => s.openSerial);
  const openLocal = useTerminals((s) => s.openLocal);

  const [protocol, setProtocol] = useState<QuickProtocol>(savedProtocol);
  const [text, setText] = useState("");
  const [cursor, setCursor] = useState(0);

  // Serial form.
  const [ports, setPorts] = useState<SerialPortInfo[]>([]);
  const [loadingPorts, setLoadingPorts] = useState(false);
  const [device, setDevice] = useState("");
  const [baud, setBaud] = useState(115200);
  const [advanced, setAdvanced] = useState(false);
  const [dataBits, setDataBits] = useState(8);
  const [parity, setParity] = useState("none");
  const [stopBits, setStopBits] = useState(1);

  const loadPorts = () => {
    setLoadingPorts(true);
    void directApi
      .serialPorts()
      .then((list) => {
        setPorts(list);
        // Pre-select the first device so one Enter is enough.
        setDevice((cur) => cur || list[0]?.path || "");
      })
      .catch(() => setPorts([]))
      .finally(() => setLoadingPorts(false));
  };

  useEffect(() => {
    if (protocol === "serial" && ports.length === 0) loadPorts();
    // Deliberately not depending on ports: loadPorts sets it and we would loop.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [protocol]);

  const q = text.trim().toLowerCase();
  const matches = useMemo(
    () =>
      hosts
        .filter(
          (h) =>
            !q ||
            [h.name, h.address, h.username, ...h.tags].some((f) => f.toLowerCase().includes(q)),
        )
        .slice(0, 8),
    [hosts, q],
  );

  const sshTarget = parseTarget(text, "root");
  const telnetTarget = parseTelnet(text);

  const go = (fn: () => void) => {
    fn();
    useApp.setState({ page: { kind: "terminal" } });
    onClose();
  };

  const connectTelnet = () => {
    if (!telnetTarget) return;
    const label = `${telnetTarget.host}:${telnetTarget.port}`;
    go(() => {
      addRecent(TELNET_SCHEME + label);
      openTelnet(telnetTarget);
    });
  };

  const connectSerial = () => {
    if (!device.trim()) return;
    go(() =>
      openSerial({
        path: device.trim(),
        baud,
        dataBits,
        parity,
        stopBits,
      }),
    );
  };

  type Row = { label: string; sub: string; icon: React.ReactNode; run: () => void };

  const rows: Row[] = [];

  if (protocol === "ssh") {
    if (sshTarget && text.includes("@")) {
      rows.push({
        label: `${sshTarget.username}@${sshTarget.address}${
          sshTarget.port !== 22 ? `:${sshTarget.port}` : ""
        }`,
        sub: "快速连接，不保存",
        icon: <IconTerminal size={15} />,
        run: () =>
          go(() => {
            addRecent(text.trim());
            openTarget(sshTarget);
          }),
      });
    }
    for (const h of matches) {
      rows.push({
        label: h.name || h.address,
        sub:
          h.protocol === "serial"
            ? h.address
            : `${h.username}@${h.address}:${h.port}`,
        icon: <IconServer size={15} />,
        run: () => go(() => openHost(h)),
      });
    }
    // Bare SSH recents. Anything with a scheme belongs to another protocol.
    for (const r of recent.filter((r) => !r.includes("://"))) {
      if (q && !r.toLowerCase().includes(q)) continue;
      const t = parseTarget(r, "root");
      if (t)
        rows.push({
          label: r,
          sub: "最近连接",
          icon: <IconClock size={15} />,
          run: () => go(() => openTarget(t)),
        });
    }
    // Unchanged from before: the local shell is reachable by typing "local".
    if (!q || "local 本地".includes(q)) {
      rows.push({
        label: "本地终端",
        sub: "在这台电脑上打开 shell",
        icon: <IconTerminal size={15} />,
        run: () => go(() => openLocal()),
      });
    }
  }

  if (protocol === "telnet") {
    if (telnetTarget) {
      rows.push({
        label: `${telnetTarget.host}:${telnetTarget.port}`,
        sub: "Telnet 快速连接（明文）",
        icon: <IconGlobe size={15} />,
        run: connectTelnet,
      });
    }
    for (const h of matches.filter((h) => h.protocol === "telnet")) {
      rows.push({
        label: h.name || h.address,
        sub: `${h.address}:${h.port}`,
        icon: <IconServer size={15} />,
        run: () => go(() => openHost(h)),
      });
    }
    for (const r of recent.filter((r) => r.startsWith(TELNET_SCHEME))) {
      const label = r.slice(TELNET_SCHEME.length);
      if (q && !label.toLowerCase().includes(q)) continue;
      const t = parseTelnet(label);
      if (t)
        rows.push({
          label,
          sub: "最近连接",
          icon: <IconClock size={15} />,
          run: () => go(() => openTelnet(t)),
        });
    }
  }

  const sel = Math.min(cursor, Math.max(rows.length - 1, 0));

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(c + 1, Math.max(rows.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(c - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (rows[sel]) rows[sel].run();
      else if (protocol === "ssh" && sshTarget)
        go(() => {
          addRecent(text.trim());
          openTarget(sshTarget);
        });
      else if (protocol === "telnet") connectTelnet();
    }
  };

  const placeholder =
    protocol === "ssh"
      ? "user@host:port，或搜索已保存的主机"
      : protocol === "telnet"
        ? "host 或 host:port（默认 23）"
        : "";

  return (
    <Modal onClose={onClose} width={560} className="palette">
      <div className="palette__head">
        <Segmented
          value={protocol}
          options={PROTOCOLS.map((p) => ({ value: p.value, label: p.label }))}
          onChange={(p) => {
            setProtocol(p);
            setPref("quickProtocol", p);
            setText("");
            setCursor(0);
          }}
        />
      </div>

      {protocol === "serial" ? (
        <div className="qserial">
          <label className="qserial__row">
            <span className="qserial__label">设备</span>
            <div className="qserial__control">
              <div className="qserial__devices">
                <input
                  className="input"
                  autoFocus
                  placeholder="/dev/tty.usbserial-1420"
                  value={device}
                  onChange={(e) => setDevice(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") connectSerial();
                  }}
                  spellCheck={false}
                />
                <button
                  className="iconbtn"
                  title="刷新设备列表"
                  onClick={loadPorts}
                  disabled={loadingPorts}
                >
                  <IconRefresh size={14} />
                </button>
              </div>
              <select
                className="input qserial__pick"
                value=""
                onChange={(e) => {
                  if (e.target.value) setDevice(e.target.value);
                }}
              >
                <option value="">
                  {loadingPorts
                    ? "正在检测…"
                    : ports.length
                      ? `检测到 ${ports.length} 个设备`
                      : "未检测到串口设备"}
                </option>
                {ports.map((p) => (
                  <option key={p.path} value={p.path}>
                    {p.name || p.path}
                  </option>
                ))}
              </select>
            </div>
          </label>

          <label className="qserial__row">
            <span className="qserial__label">波特率</span>
            <div className="qserial__control">
              <select
                className="input"
                value={baud}
                onChange={(e) => setBaud(Number(e.target.value))}
              >
                {BAUD_RATES.map((b) => (
                  <option key={b} value={b}>
                    {b}
                  </option>
                ))}
              </select>
            </div>
          </label>

          <button className="qserial__advanced" onClick={() => setAdvanced(!advanced)}>
            {advanced ? "收起高级选项" : "高级选项（数据位 / 校验 / 停止位）"}
          </button>

          {advanced && (
            <div className="qserial__advgrid">
              <label className="qserial__row">
                <span className="qserial__label">数据位</span>
                <select
                  className="input"
                  value={dataBits}
                  onChange={(e) => setDataBits(Number(e.target.value))}
                >
                  {[5, 6, 7, 8].map((b) => (
                    <option key={b} value={b}>
                      {b}
                    </option>
                  ))}
                </select>
              </label>
              <label className="qserial__row">
                <span className="qserial__label">校验</span>
                <select className="input" value={parity} onChange={(e) => setParity(e.target.value)}>
                  <option value="none">无</option>
                  <option value="odd">奇</option>
                  <option value="even">偶</option>
                </select>
              </label>
              <label className="qserial__row">
                <span className="qserial__label">停止位</span>
                <select
                  className="input"
                  value={stopBits}
                  onChange={(e) => setStopBits(Number(e.target.value))}
                >
                  <option value={1}>1</option>
                  <option value={2}>2</option>
                </select>
              </label>
            </div>
          )}

          <div className="qserial__footer">
            <button className="btn btn--primary" onClick={connectSerial} disabled={!device.trim()}>
              <IconUsb size={14} /> 连接
            </button>
          </div>
        </div>
      ) : protocol === "local" ? (
        <div className="palette__local">
          <div className="palette__localTitle">在这台电脑上打开 shell</div>
          <div className="palette__localHint">
            使用你的登录 shell（Windows 上是 PowerShell）。不需要网络，也不经过任何服务器。
          </div>
          <button className="btn btn--primary" onClick={() => go(() => openLocal())}>
            <IconTerminal size={14} /> 打开本地终端
          </button>
        </div>
      ) : (
        <>
          <input
            className="palette__input"
            autoFocus
            placeholder={placeholder}
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setCursor(0);
            }}
            onKeyDown={onKeyDown}
          />
          <div className="palette__list">
            {rows.length === 0 && (
              <div className="palette__empty">
                {text
                  ? protocol === "telnet"
                    ? "回车直接连接这个地址。"
                    : "没有匹配的主机。输入 user@host 直接连接。"
                  : protocol === "telnet"
                    ? "输入 host 或 host:port，回车连接。"
                    : "没有保存的主机。"}
              </div>
            )}
            {rows.map((r, i) => (
              <button
                key={i}
                className={`palette__row ${i === sel ? "palette__row--on" : ""}`}
                onMouseEnter={() => setCursor(i)}
                onClick={r.run}
              >
                <span className="palette__icon">{r.icon}</span>
                <span className="palette__main">{r.label}</span>
                <span className="palette__sub">{r.sub}</span>
                {i === sel && <kbd className="palette__kbd">↵</kbd>}
              </button>
            ))}
          </div>
          {protocol === "telnet" && (
            <div className="palette__warn">Telnet 是明文协议，密码和内容都可能被同网段的人看到。</div>
          )}
        </>
      )}
    </Modal>
  );
}
