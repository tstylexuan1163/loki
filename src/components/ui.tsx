/**
 * Shared UI primitives: modal shell, toasts, a promise-based confirm dialog and
 * a small popover menu.
 *
 * Toasts and confirms live in module-level stores so any feature can call
 * `toast(...)` or `await confirmDialog(...)` without threading props.
 */
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { create } from "zustand";
import { IconAlert, IconCheck, IconInfo, IconX } from "@/components/icons";

// ---- Toasts ----------------------------------------------------------------

type ToastKind = "info" | "success" | "error";

interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

const useToasts = create<{ items: Toast[] }>(() => ({ items: [] }));
let toastSeq = 0;

export function toast(text: string, kind: ToastKind = "info") {
  const id = ++toastSeq;
  useToasts.setState((s) => ({ items: [...s.items.slice(-3), { id, kind, text }] }));
  // Errors stay longer: they are the ones people need to read.
  window.setTimeout(
    () => useToasts.setState((s) => ({ items: s.items.filter((t) => t.id !== id) })),
    kind === "error" ? 7000 : 3200,
  );
}

export function Toasts() {
  const items = useToasts((s) => s.items);
  return (
    <div className="toasts" role="status" aria-live="polite">
      {items.map((t) => (
        <div key={t.id} className={`toast toast--${t.kind}`}>
          <span className="toast__icon">
            {t.kind === "success" ? <IconCheck size={15} /> : t.kind === "error" ? <IconAlert size={15} /> : <IconInfo size={15} />}
          </span>
          <span className="toast__text">{t.text}</span>
          <button
            className="toast__close"
            onClick={() => useToasts.setState((s) => ({ items: s.items.filter((x) => x.id !== t.id) }))}
            aria-label="关闭"
          >
            <IconX size={13} />
          </button>
        </div>
      ))}
    </div>
  );
}

// ---- Confirm ---------------------------------------------------------------

interface ConfirmRequest {
  title: string;
  body?: ReactNode;
  confirmLabel?: string;
  danger?: boolean;
  resolve: (ok: boolean) => void;
}

const useConfirm = create<{ current: ConfirmRequest | null }>(() => ({ current: null }));

/** Resolves `true` when the user confirms. */
export function confirmDialog(opts: Omit<ConfirmRequest, "resolve">): Promise<boolean> {
  return new Promise((resolve) => {
    useConfirm.setState({ current: { ...opts, resolve } });
  });
}

export function ConfirmHost() {
  const current = useConfirm((s) => s.current);
  if (!current) return null;
  const done = (ok: boolean) => {
    current.resolve(ok);
    useConfirm.setState({ current: null });
  };
  return (
    <Modal title={current.title} onClose={() => done(false)} width={420}>
      {current.body && <div className="modal__body-text">{current.body}</div>}
      <div className="footerbar">
        <button className="btn btn--ghost" onClick={() => done(false)}>
          取消
        </button>
        <button
          className={`btn ${current.danger ? "btn--danger" : "btn--primary"}`}
          autoFocus
          onClick={() => done(true)}
        >
          {current.confirmLabel ?? "确定"}
        </button>
      </div>
    </Modal>
  );
}

// ---- Modal -----------------------------------------------------------------

export function Modal({
  title,
  subtitle,
  onClose,
  width = 480,
  children,
  className = "",
}: {
  title?: ReactNode;
  subtitle?: ReactNode;
  onClose: () => void;
  width?: number | string;
  children: ReactNode;
  className?: string;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div
        className={`modal ${className}`}
        style={{ width }}
        role="dialog"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {(title || subtitle) && (
          <div className="modal__head">
            <div>
              {title && <h2 className="modal__title">{title}</h2>}
              {subtitle && <div className="modal__subtitle">{subtitle}</div>}
            </div>
            <button className="iconbtn" onClick={onClose} aria-label="关闭">
              <IconX size={15} />
            </button>
          </div>
        )}
        {children}
      </div>
    </div>
  );
}

// ---- Menu ------------------------------------------------------------------

export interface MenuItem {
  label: string;
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  hint?: string;
  onSelect: () => void;
}

/** A popover menu anchored at a point; closes on outside click or Escape. */
export function Menu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: (MenuItem | "sep")[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });

  // Keep the menu on screen near the right and bottom edges.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      x: Math.min(x, window.innerWidth - r.width - 8),
      y: Math.min(y, window.innerHeight - r.height - 8),
    });
  }, [x, y]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  return (
    <div ref={ref} className="menu" style={{ left: pos.x, top: pos.y }} role="menu">
      {items.map((it, i) =>
        it === "sep" ? (
          <div key={`sep-${i}`} className="menu__sep" />
        ) : (
          <button
            key={it.label}
            className={`menu__item ${it.danger ? "menu__item--danger" : ""}`}
            disabled={it.disabled}
            role="menuitem"
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            <span className="menu__icon">{it.icon}</span>
            <span className="menu__label">{it.label}</span>
            {it.hint && <span className="menu__hint">{it.hint}</span>}
          </button>
        ),
      )}
    </div>
  );
}

/** State helper for a menu opened from a click or right-click. */
export function useMenu<T = undefined>() {
  const [menu, setMenu] = useState<{ x: number; y: number; data: T } | null>(null);
  return {
    menu,
    open: (e: React.MouseEvent, data: T) => {
      e.preventDefault();
      e.stopPropagation();
      setMenu({ x: e.clientX, y: e.clientY, data });
    },
    openBelow: (el: HTMLElement, data: T) => {
      const r = el.getBoundingClientRect();
      setMenu({ x: r.left, y: r.bottom + 4, data });
    },
    close: () => setMenu(null),
  };
}

// ---- Small controls --------------------------------------------------------

export function Switch({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className={`switch ${checked ? "switch--on" : ""}`}
      onClick={() => onChange(!checked)}
    >
      <span className="switch__knob" />
    </button>
  );
}

/** Segmented control for 2–4 mutually exclusive options. */
export function Segmented<T extends string | number>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="segmented">
      {options.map((o) => (
        <button
          key={String(o.value)}
          type="button"
          className={`segmented__btn ${o.value === value ? "segmented__btn--on" : ""}`}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
