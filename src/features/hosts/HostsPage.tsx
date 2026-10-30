import { useMemo, useState } from "react";
import logoUrl from "@/assets/logo.png";
import { useApp } from "@/app/store";
import { usePrefs } from "@/app/prefs";
import { useTerminals } from "@/features/terminal/terminals";
import { QuickConnect } from "@/features/terminal/QuickConnect";
import { ImportDialog } from "@/features/hosts/ImportDialog";
import { CategoryManager } from "@/features/hosts/CategoryManager";
import { Menu, confirmDialog, toast, useMenu } from "@/components/ui";
import {
  IconBolt,
  IconChevronRight,
  IconCopy,
  IconEdit,
  IconGlobe,
  IconGrid,
  IconImport,
  IconKey,
  IconLink,
  IconList,
  IconMore,
  IconPlug,
  IconPlus,
  IconSearch,
  IconTag,
  IconTerminal,
  IconTrash,
  IconUsb,
  IconX,
} from "@/components/icons";
import {
  api,
  categoryColor,
  errorMessage,
  sortedCategories,
  tagColor,
  type CategoryView,
  type HostView,
} from "@/lib/api";
import { keyLabel } from "@/app/platform";

/** `user@host:port`, or the device path and baud for a serial console. */
function describe(h: HostView): string {
  if (h.protocol === "serial") return `${h.address} · ${h.baud ?? 115200} baud`;
  if (h.protocol === "telnet") return `${h.address}:${h.port}`;
  // A host saved without a login name asks for one when connecting.
  const user = h.username ? `${h.username}@` : "";
  return `${user}${h.address}${h.port !== 22 ? `:${h.port}` : ""}`;
}

/** A coloured monogram so a long list can be scanned by shape, not just text. */
function Avatar({ host }: { host: HostView }) {
  const icon =
    host.protocol === "serial" ? <IconUsb size={16} /> : host.protocol === "telnet" ? <IconGlobe size={16} /> : null;
  const letter = (host.name || host.address || "?").trim().charAt(0).toUpperCase();
  const hue = tagColor(host.name || host.address);
  return (
    <span className="avatar" style={{ ["--hue" as string]: hue }}>
      {icon ?? letter}
    </span>
  );
}

function AuthHints({ host, jumpName }: { host: HostView; jumpName?: string }) {
  if (host.protocol !== "ssh") return null;
  return (
    <span className="authhints">
      {(host.has_key || host.key_id) && (
        <span className="authhint" title="私钥">
          <IconKey size={12} />
        </span>
      )}
      {host.has_password && (
        <span className="authhint" title="已保存密码">
          ••
        </span>
      )}
      {jumpName && (
        <span className="authhint" title={`经跳板机 ${jumpName}`}>
          <IconLink size={12} />
        </span>
      )}
    </span>
  );
}

export function HostsPage() {
  const hosts = useApp((s) => s.hosts);
  const openNewHost = useApp((s) => s.openNewHost);
  const openHost = useApp((s) => s.openHost);
  const refreshHosts = useApp((s) => s.refreshHosts);
  const openTerminal = useTerminals((s) => s.openHost);
  const openLocal = useTerminals((s) => s.openLocal);
  const tabs = useTerminals((s) => s.tabs);
  const categories = useApp((s) => s.categories);
  const mode = usePrefs((s) => s.hostsView);
  const collapsed = usePrefs((s) => s.collapsedCategories);
  const setPref = usePrefs((s) => s.set);

  const [query, setQuery] = useState("");
  const [quick, setQuick] = useState(false);
  const [importing, setImporting] = useState(false);
  const [managing, setManaging] = useState(false);
  const menu = useMenu<HostView>();

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return hosts
      .filter(
        (h) =>
          !q ||
          [h.name, h.address, h.username, ...h.tags].some((f) => f.toLowerCase().includes(q)),
      )
      .sort((a, b) => (a.name || a.address).localeCompare(b.name || b.address, "zh-CN"));
  }, [hosts, query]);

  // One section per category, in the user's order; a host in two categories
  // shows in both. Hosts with none go last, under 未分类.
  const sections = useMemo(() => {
    const has = (h: HostView, c: CategoryView) =>
      h.tags.some((t) => t.toLowerCase() === c.name.toLowerCase());
    const out: { key: string; category: CategoryView | null; hosts: HostView[] }[] = categories
      .map((c) => ({ key: c.id, category: c, hosts: visible.filter((h) => has(h, c)) }))
      .filter((s) => s.hosts.length > 0);
    const loose = visible.filter((h) => h.tags.length === 0);
    if (loose.length) out.push({ key: "", category: null, hosts: loose });
    return out;
  }, [categories, visible]);
  const grouped = categories.length > 0;

  const toggleSection = (key: string) =>
    setPref(
      "collapsedCategories",
      collapsed.includes(key) ? collapsed.filter((k) => k !== key) : [...collapsed, key],
    );

  const liveCount = (id: string) =>
    tabs.filter((t) => t.hostId === id && t.status === "connected").length;
  const nameOf = (id: string | null) => hosts.find((h) => h.id === id)?.name;

  function connect(h: HostView) {
    openTerminal(h);
    useApp.setState({ page: { kind: "terminal" } });
  }

  async function remove(h: HostView) {
    const ok = await confirmDialog({
      title: `删除 ${h.name || h.address}？`,
      body: "主机和它保存的密码、私钥都会从保险库中删除。",
      confirmLabel: "删除",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.hostDelete(h.id);
      await refreshHosts();
      toast(`已删除 ${h.name || h.address}`, "success");
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  async function duplicate(h: HostView) {
    try {
      const id = await api.hostDuplicate(h.id);
      await refreshHosts();
      openHost(id);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  function renderHosts(list: HostView[]) {
    return mode === "cards" ? (
      <div className="hostcards">
        {list.map((h) => {
          const live = liveCount(h.id);
          return (
            <div
              key={h.id}
              className="hostcard"
              onDoubleClick={() => connect(h)}
              onContextMenu={(e) => menu.open(e, h)}
            >
              <div className="hostcard__head">
                <Avatar host={h} />
                <div className="hostcard__titles">
                  <span className="hostcard__name">
                    {h.name || "未命名"}
                    {live > 0 && <span className="livebadge" title={`${live} 个会话已连接`}>{live}</span>}
                  </span>
                  <span className="hostcard__addr">{describe(h)}</span>
                </div>
                <button className="iconbtn hostcard__more" onClick={(e) => menu.open(e, h)} aria-label="更多">
                  <IconMore size={16} />
                </button>
              </div>
              <div className="hostcard__foot">
                <div className="hostcard__tags">
                  {h.protocol !== "ssh" && <span className="pill">{h.protocol.toUpperCase()}</span>}
                  {sortedCategories(h.tags, categories).slice(0, 3).map((t) => (
                    <span key={t} className="tag tag--sm">
                      <i className="tag__dot" style={{ background: categoryColor(t, categories) }} />
                      {t}
                    </span>
                  ))}
                  <AuthHints host={h} jumpName={nameOf(h.jump_host_id)} />
                </div>
                <button className="btn btn--sm btn--primary hostcard__connect" onClick={() => connect(h)}>
                  连接
                </button>
              </div>
            </div>
          );
        })}
      </div>
    ) : (
      <div className="hosttable">
        <div className="hostrow hostrow--head">
          <span>名称</span>
          <span>地址</span>
          <span>分类</span>
          <span />
        </div>
        {list.map((h) => (
          <div
            key={h.id}
            className="hostrow"
            onDoubleClick={() => connect(h)}
            onContextMenu={(e) => menu.open(e, h)}
          >
            <div className="hostrow__name">
              <Avatar host={h} />
              <span>{h.name || "未命名"}</span>
              {liveCount(h.id) > 0 && <span className="livebadge">{liveCount(h.id)}</span>}
              <AuthHints host={h} jumpName={nameOf(h.jump_host_id)} />
            </div>
            <div className="hostrow__addr">{describe(h)}</div>
            <div className="tags">
              {h.protocol !== "ssh" && <span className="pill">{h.protocol.toUpperCase()}</span>}
              {sortedCategories(h.tags, categories).map((t) => (
                <span key={t} className="tag tag--sm">
                  <i className="tag__dot" style={{ background: categoryColor(t, categories) }} />
                  {t}
                </span>
              ))}
            </div>
            <div className="hostrow__actions">
              <button className="btn btn--sm btn--primary" onClick={() => connect(h)}>
                连接
              </button>
              <button className="iconbtn" onClick={(e) => menu.open(e, h)} aria-label="更多">
                <IconMore size={16} />
              </button>
            </div>
          </div>
        ))}
      </div>
    );
  }

  const header = (
    <div className="pagehead">
      <div className="pagehead__title">
        主机
        {hosts.length > 0 && <span className="pagehead__count">{hosts.length}</span>}
      </div>
      <div className="spacer" />
      <button className="btn btn--ghost" onClick={() => setImporting(true)} title="从 ~/.ssh/config 导入">
        <IconImport size={14} /> 导入
      </button>
      <button className="btn" onClick={() => setQuick(true)} title={`快速连接 (${keyLabel("K")})`}>
        <IconBolt size={14} /> 快速连接
      </button>
      <button className="btn btn--primary" onClick={openNewHost}>
        <IconPlus size={14} /> 新建主机
      </button>
    </div>
  );

  const dialogs = (
    <>
      {quick && <QuickConnect onClose={() => setQuick(false)} />}
      {importing && <ImportDialog onClose={() => setImporting(false)} />}
      {managing && <CategoryManager onClose={() => setManaging(false)} />}
      {menu.menu && (
        <Menu
          x={menu.menu.x}
          y={menu.menu.y}
          onClose={menu.close}
          items={[
            { label: "连接", icon: <IconPlug size={14} />, onSelect: () => connect(menu.menu!.data) },
            { label: "编辑", icon: <IconEdit size={14} />, onSelect: () => openHost(menu.menu!.data.id) },
            { label: "复制主机", icon: <IconCopy size={14} />, onSelect: () => void duplicate(menu.menu!.data) },
            {
              label: "复制地址",
              icon: <IconCopy size={14} />,
              onSelect: () => {
                void navigator.clipboard.writeText(describe(menu.menu!.data));
                toast("已复制");
              },
            },
            "sep",
            { label: "删除", danger: true, icon: <IconTrash size={14} />, onSelect: () => void remove(menu.menu!.data) },
          ]}
        />
      )}
    </>
  );

  if (hosts.length === 0) {
    return (
      <div className="page">
        {header}
        <div className="content content--center">
          <div className="empty">
            <img className="empty__mark" src={logoUrl} alt="" />
            <div className="empty__title">还没有主机</div>
            <div className="empty__hint">
              添加第一台服务器，或者从 <code>~/.ssh/config</code> 一键导入。地址、用户名和密码都会在本机加密后才落盘。
            </div>
            <div className="empty__actions">
              <button className="btn btn--primary" onClick={openNewHost}>
                <IconPlus size={14} /> 新建主机
              </button>
              <button className="btn" onClick={() => setImporting(true)}>
                <IconImport size={14} /> 从 ssh config 导入
              </button>
              <button className="btn btn--ghost" onClick={() => {
                openLocal();
                useApp.setState({ page: { kind: "terminal" } });
              }}>
                <IconTerminal size={14} /> 本地终端
              </button>
            </div>
          </div>
        </div>
        {dialogs}
      </div>
    );
  }

  return (
    <div className="page">
      {header}
      <div className="toolbar">
        <label className="searchbox">
          <IconSearch size={14} />
          <input
            placeholder="搜索名称、地址、用户或分类"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query && (
            <button className="iconbtn" onClick={() => setQuery("")} aria-label="清除">
              <IconX size={12} />
            </button>
          )}
        </label>
        <div className="spacer" />
        <button className="btn btn--ghost" onClick={() => setManaging(true)}>
          <IconTag size={14} /> 管理分类
        </button>
        <div className="segmented">
          <button
            className={`segmented__btn ${mode === "cards" ? "segmented__btn--on" : ""}`}
            onClick={() => setPref("hostsView", "cards")}
            title="卡片"
          >
            <IconGrid size={14} />
          </button>
          <button
            className={`segmented__btn ${mode === "list" ? "segmented__btn--on" : ""}`}
            onClick={() => setPref("hostsView", "list")}
            title="列表"
          >
            <IconList size={14} />
          </button>
        </div>
      </div>

      <div className="content">
        {visible.length === 0 && (
          <div className="muted" style={{ padding: "24px 4px" }}>
            没有匹配的主机。
          </div>
        )}

        {grouped
          ? sections.map((sec) => {
              const folded = collapsed.includes(sec.key) && !query.trim();
              const color = sec.category ? categoryColor(sec.category.name, categories) : "var(--text-faint)";
              return (
                <section key={sec.key || "none"} className="hostgroup">
                  <button className="hostgroup__head" onClick={() => toggleSection(sec.key)}>
                    <IconChevronRight size={13} className={`hostgroup__chev ${folded ? "" : "hostgroup__chev--open"}`} />
                    <i className="tag__dot" style={{ background: color }} />
                    <span className="hostgroup__name">{sec.category?.name ?? "未分类"}</span>
                    <span className="hostgroup__count">{sec.hosts.length}</span>
                  </button>
                  {!folded && renderHosts(sec.hosts)}
                </section>
              );
            })
          : renderHosts(visible)}
      </div>
      {dialogs}
    </div>
  );
}

