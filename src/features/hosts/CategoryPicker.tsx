import { useMemo, useState } from "react";
import { useApp } from "@/app/store";
import { categoryColor } from "@/lib/api";
import { IconCheck } from "@/components/icons";

/** Picks any number of categories, and creates new ones by typing a name. */
export function CategoryPicker({
  value,
  onChange,
}: {
  value: string[];
  onChange: (tags: string[]) => void;
}) {
  const categories = useApp((s) => s.categories);
  const [draft, setDraft] = useState("");

  // Every known category, plus names picked here that are not saved yet.
  const choices = useMemo(() => {
    const names = categories.map((c) => c.name);
    for (const t of value) if (!names.some((n) => n.toLowerCase() === t.toLowerCase())) names.push(t);
    return names;
  }, [categories, value]);

  function toggle(t: string) {
    onChange(
      value.some((x) => x.toLowerCase() === t.toLowerCase())
        ? value.filter((x) => x.toLowerCase() !== t.toLowerCase())
        : [...value, t],
    );
  }

  function commit() {
    const t = draft.trim().replace(/,$/, "");
    // Typing an existing name selects it rather than creating a twin.
    const known = choices.find((c) => c.toLowerCase() === t.toLowerCase());
    if (t && !value.some((x) => x.toLowerCase() === t.toLowerCase())) onChange([...value, known ?? t]);
    setDraft("");
  }

  return (
    <div className="catpick">
      {choices.map((t) => {
        const on = value.some((x) => x.toLowerCase() === t.toLowerCase());
        return (
          <button
            key={t}
            type="button"
            className={`chip ${on ? "chip--on" : ""}`}
            aria-pressed={on}
            onClick={() => toggle(t)}
          >
            <i className="tag__dot" style={{ background: categoryColor(t, categories) }} />
            {t}
            {on && <IconCheck size={11} />}
          </button>
        );
      })}
      <input
        className="catpick__new"
        placeholder="新建分类，回车添加"
        maxLength={32}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            commit();
          }
        }}
      />
    </div>
  );
}
