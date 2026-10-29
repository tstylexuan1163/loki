import { useState } from "react";
import { api, errorMessage } from "@/lib/api";

export function ChangePasswordSection() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  const mismatch = confirm.length > 0 && next !== confirm;
  const canSubmit = current.length > 0 && next.length >= 8 && next === confirm;

  async function submit() {
    if (!canSubmit || busy) return;
    setBusy(true);
    setError(null);
    setDone(false);
    try {
      // Re-deriving is the expensive part; the old password is verified as a
      // side effect of unlocking again, so a wrong one fails here.
      await api.vaultUnlock(current);
      await api.vaultChangePassword(next);
      setCurrent("");
      setNext("");
      setConfirm("");
      setDone(true);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="editor__section">
      <div className="editor__legend">修改主密码</div>

      <div className="field">
        <label className="field__label">当前密码</label>
        <input
          className="field__input"
          type="password"
          value={current}
          onChange={(e) => {
            setCurrent(e.target.value);
            setDone(false);
          }}
        />
      </div>

      <div className="field">
        <label className="field__label">新密码</label>
        <input
          className="field__input"
          type="password"
          placeholder="至少 8 位"
          value={next}
          onChange={(e) => {
            setNext(e.target.value);
            setDone(false);
          }}
        />
      </div>

      <div className="field">
        <label className="field__label">确认新密码</label>
        <input
          className="field__input"
          type="password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
        />
        {mismatch && <div className="error">两次输入不一致</div>}
      </div>

      <div className="muted" style={{ lineHeight: 1.6 }}>
        改密码只会重新加密保险库密钥，不会重写任何条目，所以很快。恢复码继续有效。
      </div>

      {error && <div className="error">{error}</div>}
      {done && <div className="notice">主密码已更新。</div>}

      <div className="footerbar" style={{ marginTop: 14 }}>
        <button className="btn btn--primary" disabled={!canSubmit || busy} onClick={() => void submit()}>
          {busy ? "处理中…" : "更新主密码"}
        </button>
      </div>
    </div>
  );
}
