import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useApp } from "@/app/store";
import { Modal, Segmented, toast } from "@/components/ui";
import { CategoryPicker } from "@/features/hosts/CategoryPicker";
import { errorMessage, scanApi, type ScanEvent, type ScanHit } from "@/lib/api";

type Proto = "ssh" | "telnet";

const DEFAULT_TEMPLATE = "{vendor}-{ip}";

/** Same rules as `netscan::render_name`, for the live preview only. */
function previewName(template: string, address: string, vendor: string | null) {
  const last = address.split(".").pop() ?? address;
  const name = template
    .replaceAll("{ip}", address)
    .replaceAll("{last}", last)
    .replaceAll("{vendor}", vendor ?? "")
    .replace(/^[\s_-]+|[\s_-]+$/g, "")
    .replaceAll("--", "-")
    .replaceAll("__", "_");
  return name || address;
}

/**
 * Scans a subnet for SSH and telnet, then saves the chosen devices as hosts
 * with one login and one set of categories: a switch management VLAN in a
 * single pass instead of one host editor per switch.
 */
export function ScanDialog({ onClose }: { onClose: () => void }) {
  const hosts = useApp((s) => s.hosts);
  const refreshHosts = useApp((s) => s.refreshHosts);

  const [targets, setTargets] = useState("");
  const [useSsh, setUseSsh] = useState(true);
  const [useTelnet, setUseTelnet] = useState(true);
  const [sshPort, setSshPort] = useState("22");
  const [telnetPort, setTelnetPort] = useState("23");

  const [phase, setPhase] = useState<"idle" | "scanning" | "done" | "stopped">("idle");
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [hits, setHits] = useState<ScanHit[]>([]);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [proto, setProto] = useState<Record<string, Proto>>({});
  const [error, setError] = useState<string | null>(null);

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [tags, setTags] = useState<string[]>([]);
  const [template, setTemplate] = useState(DEFAULT_TEMPLATE);
  const [busy, setBusy] = useState(false);

  const scanId = useRef("");
  // Addresses this scan has already listed, so the final result does not
  // re-tick a device the user unticked while the scan was running.
  const seen = useRef(new Set<string>());
  const scanning = phase === "scanning";

  // Addresses already saved; a hit on one starts unticked.
  const saved = useMemo(() => {
    const m = new Map<string, string>();
    for (const h of hosts) if (!m.has(h.address)) m.set(h.address, h.name);
    return m;
  }, [hosts]);

  function addHit(hit: ScanHit) {
    if (seen.current.has(hit.address)) return;
    seen.current.add(hit.address);
    setHits((cur) => [...cur, hit]);
    setProto((cur) => ({ ...cur, [hit.address]: hit.ssh ? "ssh" : "telnet" }));
    if (!saved.has(hit.address)) setPicked((cur) => new Set(cur).add(hit.address));
  }

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let gone = false;
    void listen<ScanEvent>("netscan-event", (e) => {
      const ev = e.payload;
      if (ev.scanId !== scanId.current) return;
      if (ev.type === "progress") setProgress({ done: ev.done, total: ev.total });
      else addHit(ev.hit);
    }).then((u) => (gone ? u() : (unlisten = u)));
    return () => {
      gone = true;
      unlisten?.();
    };
    // addHit only reads refs and `saved`, which does not change while the
    // dialog is open.
  }, []);

  // Closing the dialog mid-scan should not leave sockets open behind it.
  useEffect(() => () => void (scanId.current && scanApi.cancel()), []);

  async function start() {
    const port = (s: string) => {
      const n = Number(s);
      return Number.isInteger(n) && n > 0 && n < 65536 ? n : null;
    };
    if ((useSsh && !port(sshPort)) || (useTelnet && !port(telnetPort))) {
      setError("端口号应在 1–65535 之间");
      return;
    }
    const id = crypto.randomUUID();
    scanId.current = id;
    seen.current = new Set();
    setError(null);
    setHits([]);
    setPicked(new Set());
    setProgress({ done: 0, total: 0 });
    setPhase("scanning");
    try {
      const list = await scanApi.run({
        scanId: id,
        targets,
        sshPort: useSsh ? port(sshPort) : null,
        telnetPort: useTelnet ? port(telnetPort) : null,
      });
      if (scanId.current !== id) return;
      // Events normally delivered every hit already; this catches any that
      // arrived before the listener was ready.
      for (const h of list) addHit(h);
      setPhase((p) => (p === "stopped" ? "stopped" : "done"));
    } catch (e) {
      if (scanId.current !== id) return;
      setError(errorMessage(e));
      setPhase("idle");
    }
  }

  async function stop() {
    setPhase("stopped");
    await scanApi.cancel();
  }

  const sorted = useMemo(() => {
    const key = (a: string) => a.split(".").reduce((n, o) => n * 256 + Number(o), 0);
    return [...hits].sort((a, b) => key(a.address) - key(b.address));
  }, [hits]);

  const chosen = sorted.filter((h) => picked.has(h.address));
  const hasSshChosen = chosen.some((h) => proto[h.address] === "ssh");
  const sample = chosen[0] ?? sorted[0];

  function toggle(address: string) {
    const next = new Set(picked);
    if (next.has(address)) next.delete(address);
    else next.add(address);
    setPicked(next);
  }

  async function add() {
    setBusy(true);
    try {
      const report = await scanApi.import({
        devices: chosen.map((h) => {
          const p = proto[h.address] ?? (h.ssh ? "ssh" : "telnet");
          return {
            address: h.address,
            protocol: p,
            port: Number(p === "ssh" ? sshPort : telnetPort),
            vendor: h.vendor,
          };
        }),
        username,
        password: password || null,
        tags,
        nameTemplate: template,
      });
      // Also reloads categories, which may have gained a new one here.
      await refreshHosts();
      toast(`已添加 ${report.hosts} 台设备`, "success");
      for (const n of report.notes) toast(n);
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  const pct = progress.total ? Math.round((progress.done / progress.total) * 100) : 0;

  return (
    <Modal
      title="扫描网段"
      subtitle="找出网段里开放 SSH 或 Telnet 的设备，勾选后用同一组登录信息批量添加。"
      onClose={onClose}
      width={720}
    >
      <form
        className="scanform"
        onSubmit={(e) => {
          e.preventDefault();
          if (!scanning) void start();
        }}
      >
        <input
          className="field__input mono"
          placeholder="192.168.1.0/24，或 10.1.100.1-254，多个用逗号分隔"
          value={targets}
          autoFocus
          spellCheck={false}
          onChange={(e) => setTargets(e.target.value)}
        />
        {scanning ? (
          <button type="button" className="btn" onClick={() => void stop()}>
            停止
          </button>
        ) : (
          <button
            type="submit"
            className="btn btn--primary"
            disabled={!targets.trim() || (!useSsh && !useTelnet)}
          >
            {phase === "idle" ? "开始扫描" : "重新扫描"}
          </button>
        )}
      </form>
      <div className="scanopts">
        <label className="check">
          <input type="checkbox" checked={useSsh} onChange={(e) => setUseSsh(e.target.checked)} />
          SSH
        </label>
        <input
          className="field__input scanopts__port mono"
          value={sshPort}
          disabled={!useSsh}
          onChange={(e) => setSshPort(e.target.value)}
          aria-label="SSH 端口"
        />
        <label className="check">
          <input type="checkbox" checked={useTelnet} onChange={(e) => setUseTelnet(e.target.checked)} />
          Telnet
        </label>
        <input
          className="field__input scanopts__port mono"
          value={telnetPort}
          disabled={!useTelnet}
          onChange={(e) => setTelnetPort(e.target.value)}
          aria-label="Telnet 端口"
        />
        <span className="scanopts__hint">只探测这两个端口，不发送 ping；一个 /24 通常几秒扫完。</span>
      </div>

      {error && <div className="error">{error}</div>}

      {phase !== "idle" && (
        <div className="scanstatus">
          <div className="progress">
            <span className="progress__fill" style={{ width: `${phase === "done" ? 100 : pct}%` }} />
          </div>
          <div className="scanstatus__text muted">
            {scanning
              ? `正在扫描 ${progress.done} / ${progress.total || "…"}`
              : phase === "stopped"
                ? `已停止，扫描了 ${progress.done} / ${progress.total} 个地址`
                : `扫描完成，共 ${progress.total} 个地址`}
            {" · "}发现 {hits.length} 台设备
          </div>
        </div>
      )}

      {sorted.length > 0 && (
        <>
          <div className="importlist__bar">
            <label className="check">
              <input
                type="checkbox"
                checked={picked.size === sorted.length}
                onChange={(e) =>
                  setPicked(e.target.checked ? new Set(sorted.map((h) => h.address)) : new Set())
                }
              />
              全选
            </label>
            <span className="muted">已选 {picked.size} / {sorted.length}</span>
          </div>
          <div className="importlist">
            {sorted.map((h) => {
              const existing = saved.get(h.address);
              return (
                <div key={h.address} className={`scanrow ${existing ? "importrow--dim" : ""}`}>
                  <label className="scanrow__pick">
                    <input
                      type="checkbox"
                      checked={picked.has(h.address)}
                      onChange={() => toggle(h.address)}
                    />
                    <span className="mono">{h.address}</span>
                  </label>
                  <span>
                    {h.ssh && h.telnet ? (
                      <Segmented<Proto>
                        value={proto[h.address] ?? "ssh"}
                        options={[
                          { value: "ssh", label: "SSH" },
                          { value: "telnet", label: "Telnet" },
                        ]}
                        onChange={(v) => setProto((cur) => ({ ...cur, [h.address]: v }))}
                      />
                    ) : (
                      <span className="pill">{h.ssh ? "SSH" : "Telnet"}</span>
                    )}
                  </span>
                  <span className="scanrow__banner" title={h.banner ?? undefined}>
                    {h.vendor && <span className="pill pill--accent">{h.vendor}</span>}
                    <span className="mono">{h.banner ?? ""}</span>
                  </span>
                  <span>
                    {existing && (
                      <span className="pill pill--warn" title={`已保存为「${existing}」`}>
                        已存在
                      </span>
                    )}
                  </span>
                </div>
              );
            })}
          </div>
        </>
      )}

      {chosen.length > 0 && (
        <section className="scanadd">
          <div className="grid2">
            <div className="field">
              <label className="field__label">用户名</label>
              <input
                className="field__input"
                placeholder="可留空，连接时再填"
                autoComplete="off"
                spellCheck={false}
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
            </div>
            <div className="field">
              <label className="field__label">密码</label>
              <input
                className="field__input"
                type="password"
                placeholder="可留空，连接时再填"
                autoComplete="new-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </div>
          </div>
          <div className="field__hint scanadd__hint">
            {hasSshChosen
              ? "用户名和密码只保存给 SSH 设备，和每台主机一起加密。Telnet 设备连接后在设备提示符下登录。"
              : "Telnet 设备连接后在设备提示符下登录，不保存用户名和密码。"}
          </div>
          <div className="field">
            <label className="field__label">名称</label>
            <input
              className="field__input mono"
              value={template}
              spellCheck={false}
              onChange={(e) => setTemplate(e.target.value)}
            />
            <div className="field__hint">
              可用 <code>{"{ip}"}</code>、<code>{"{last}"}</code>（地址最后一段）、
              <code>{"{vendor}"}</code>（识别出的厂商）。
              {sample && (
                <>
                  例如 <b>{previewName(template, sample.address, sample.vendor)}</b>
                </>
              )}
            </div>
          </div>
          <div className="field">
            <label className="field__label">分类</label>
            <CategoryPicker value={tags} onChange={setTags} />
          </div>
        </section>
      )}

      <div className="footerbar">
        <button className="btn btn--ghost" onClick={onClose}>
          取消
        </button>
        <button
          className="btn btn--primary"
          disabled={busy || scanning || chosen.length === 0}
          onClick={() => void add()}
        >
          {busy ? "添加中…" : `添加 ${chosen.length} 台`}
        </button>
      </div>
    </Modal>
  );
}
