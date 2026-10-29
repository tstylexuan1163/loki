import { useApp } from "@/app/store";

/** Locking controls, split out so settings can place them on their own tab. */
export function SecuritySection() {
  const lock = useApp((s) => s.lock);

  return (
    <section className="settings__group">
      <div className="settings__group-title">锁定</div>
      <div className="settings__row">
        <div className="settings__row-main">
          <div className="settings__label">立即锁定</div>
          <div className="settings__hint">清空内存中的密钥，并断开所有终端会话。</div>
        </div>
        <button className="btn" onClick={() => void lock()}>
          锁定
        </button>
      </div>
    </section>
  );
}
