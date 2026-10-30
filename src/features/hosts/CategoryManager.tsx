import { useState } from "react";
import { useApp } from "@/app/store";
import { Modal, confirmDialog, toast } from "@/components/ui";
import { IconArrowDown, IconArrowUp, IconPlus, IconTrash } from "@/components/icons";
import { CATEGORY_COLORS, api, categoryColor, errorMessage, type CategoryView } from "@/lib/api";

/**
 * Create, rename, recolour, reorder and delete host categories.
 *
 * Every change goes straight to the vault (renames and deletes also rewrite
 * the hosts in that category), then the host list is refreshed so the page
 * behind the dialog follows along.
 */
export function CategoryManager({ onClose }: { onClose: () => void }) {
  const categories = useApp((s) => s.categories);
  const refreshHosts = useApp((s) => s.refreshHosts);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);

  async function run(action: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    try {
      await action();
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      // Also puts a refused rename back to what is stored.
      await refreshHosts();
      setBusy(false);
    }
  }

  function add() {
    const name = draft.trim();
    if (!name) return;
    void run(async () => {
      await api.categorySave({ name });
      setDraft("");
    });
  }

  function move(index: number, by: -1 | 1) {
    const ids = categories.map((c) => c.id);
    const to = index + by;
    if (to < 0 || to >= ids.length) return;
    [ids[index], ids[to]] = [ids[to], ids[index]];
    void run(() => api.categoriesReorder(ids));
  }

  async function remove(c: CategoryView) {
    if (c.count > 0) {
      const ok = await confirmDialog({
        title: `删除分类「${c.name}」？`,
        body: `其中的 ${c.count} 台主机不会被删除，只是不再属于这个分类。`,
        confirmLabel: "删除分类",
        danger: true,
      });
      if (!ok) return;
    }
    void run(() => api.categoryDelete(c.id));
  }

  return (
    <Modal
      title="管理分类"
      subtitle="一台主机可以属于多个分类；这里的顺序就是主机页和侧栏里的顺序。"
      onClose={onClose}
      width={520}
    >
      <div className="catman">
        {categories.length === 0 && <div className="muted catman__empty">还没有分类，在下面新建一个。</div>}
        {categories.map((c, i) => (
          <CategoryRow
            // Re-key on rename so the field shows the stored name afterwards.
            key={`${c.id}:${c.name}`}
            category={c}
            color={categoryColor(c.name, categories)}
            first={i === 0}
            last={i === categories.length - 1}
            busy={busy}
            onRename={(name) => void run(() => api.categorySave({ id: c.id, name, color: c.color }))}
            onColor={(color) => void run(() => api.categorySave({ id: c.id, name: c.name, color }))}
            onMove={(by) => move(i, by)}
            onDelete={() => void remove(c)}
          />
        ))}
        <form
          className="catman__add"
          onSubmit={(e) => {
            e.preventDefault();
            add();
          }}
        >
          <input
            className="field__input"
            placeholder="新分类名称，例如 交换机"
            maxLength={32}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <button className="btn btn--primary" type="submit" disabled={!draft.trim() || busy}>
            <IconPlus size={14} /> 新建
          </button>
        </form>
      </div>
    </Modal>
  );
}

function CategoryRow({
  category,
  color,
  first,
  last,
  busy,
  onRename,
  onColor,
  onMove,
  onDelete,
}: {
  category: CategoryView;
  color: string;
  first: boolean;
  last: boolean;
  busy: boolean;
  onRename: (name: string) => void;
  onColor: (color: string | null) => void;
  onMove: (by: -1 | 1) => void;
  onDelete: () => void;
}) {
  const [name, setName] = useState(category.name);
  const [picking, setPicking] = useState(false);

  const commit = () => {
    const next = name.trim();
    if (!next) setName(category.name);
    else if (next !== category.name) onRename(next);
  };

  return (
    <div className="catman__row">
      <div className="catman__swatchwrap">
        <button
          className="catman__swatch"
          style={{ background: color }}
          title="选择颜色"
          onClick={() => setPicking((v) => !v)}
        />
        {picking && (
          <div className="catman__palette" onMouseLeave={() => setPicking(false)}>
            {CATEGORY_COLORS.map((c) => (
              <button
                key={c}
                className={`catman__swatch ${category.color === c ? "catman__swatch--on" : ""}`}
                style={{ background: c }}
                onClick={() => {
                  setPicking(false);
                  onColor(c);
                }}
              />
            ))}
            <button
              className="catman__auto"
              title="按名称自动配色"
              onClick={() => {
                setPicking(false);
                onColor(null);
              }}
            >
              自动
            </button>
          </div>
        )}
      </div>
      <input
        className="catman__name"
        value={name}
        maxLength={32}
        onChange={(e) => setName(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
      />
      <span className="catman__count">{category.count} 台</span>
      <button className="iconbtn" title="上移" disabled={first || busy} onClick={() => onMove(-1)}>
        <IconArrowUp size={13} />
      </button>
      <button className="iconbtn" title="下移" disabled={last || busy} onClick={() => onMove(1)}>
        <IconArrowDown size={13} />
      </button>
      <button className="iconbtn iconbtn--danger" title="删除分类" disabled={busy} onClick={onDelete}>
        <IconTrash size={13} />
      </button>
    </div>
  );
}
