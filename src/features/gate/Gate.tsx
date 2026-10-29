import { useState } from "react";
import logoUrl from "@/assets/logo.png";
import { useApp } from "@/app/store";

/** Password strength on a 0..1 scale with a rough label. */
function strength(pw: string): { value: number; label: string; color: string } {
  if (!pw) return { value: 0, label: "", color: "transparent" };
  let score = 0;
  if (pw.length >= 8) score += 0.25;
  if (pw.length >= 14) score += 0.25;
  if (/[a-z]/.test(pw) && /[A-Z]/.test(pw)) score += 0.2;
  if (/\d/.test(pw)) score += 0.15;
  if (/[^\w\s]/.test(pw)) score += 0.15;
  const value = Math.min(1, score);
  if (value < 0.4) return { value, label: "偏弱", color: "#f2637a" };
  if (value < 0.8) return { value, label: "一般", color: "#d9b169" };
  return { value, label: "很强", color: "#2ee6a8" };
}

export function Gate() {
  const phase = useApp((s) => s.phase);
  const recoveryCode = useApp((s) => s.recoveryCode);
  const dismissRecoveryCode = useApp((s) => s.dismissRecoveryCode);

  // The code has to be acknowledged before the app becomes usable.
  if (recoveryCode) return <RecoveryCodeStep code={recoveryCode} onDone={dismissRecoveryCode} />;

  return phase === "onboarding" ? <CreateForm /> : <UnlockForm />;
}

/** Shown exactly once, right after the vault is created. */
function RecoveryCodeStep({ code, onDone }: { code: string; onDone: () => void }) {
  const [confirmed, setConfirmed] = useState(false);

  return (
    <div className="gate">
      <div className="gate__card" style={{ width: 560 }}>
        <img className="gate__mark" src={logoUrl} alt="" />
        <h1 className="gate__title">保存你的恢复码</h1>
        <p className="gate__sub">
          忘记主密码时，恢复码是找回数据的<strong>唯一</strong>方式。它只显示这一次。
        </p>

        <div
          className="fingerprint"
          style={{ fontSize: 16, letterSpacing: "0.08em", textAlign: "center" }}
        >
          {code}
        </div>

        <div className="notice">
          请抄写到纸上或存进密码管理器。<strong>本机不保存它</strong>，关闭此页面后无法再次查看。
        </div>

        <label
          style={{ display: "flex", gap: 8, alignItems: "center", fontSize: 13, marginTop: 4 }}
        >
          <input
            type="checkbox"
            checked={confirmed}
            onChange={(e) => setConfirmed(e.target.checked)}
          />
          我已将恢复码保存在安全的地方
        </label>

        <div className="footerbar" style={{ marginTop: 18 }}>
          <button className="btn btn--primary" disabled={!confirmed} onClick={onDone}>
            完成设置
          </button>
        </div>
      </div>
    </div>
  );
}

function CreateForm() {
  const createVault = useApp((s) => s.createVault);
  const busy = useApp((s) => s.busy);
  const error = useApp((s) => s.error);
  const clearError = useApp((s) => s.clearError);

  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");

  const s = strength(password);
  const mismatch = confirm.length > 0 && password !== confirm;
  const canSubmit = password.length >= 8 && password === confirm;

  async function submit() {
    if (!canSubmit || busy) return;
    const ok = await createVault(password);
    if (ok) {
      setPassword("");
      setConfirm("");
    }
  }

  return (
    <div className="gate">
      <div className="gate__card">
        <img className="gate__mark" src={logoUrl} alt="" />
        <h1 className="gate__title">欢迎使用 loki</h1>
        <p className="gate__sub">
          设置一个主密码。所有主机、密钥和终端会话都用它派生的密钥加密。
        </p>

        <div className="field">
          <label className="field__label">主密码</label>
          <input
            className="field__input"
            type="password"
            autoFocus
            value={password}
            placeholder="至少 8 位"
            onChange={(e) => {
              setPassword(e.target.value);
              clearError();
            }}
          />
          <div className="strength">
            <div
              className="strength__fill"
              style={{ width: `${s.value * 100}%`, background: s.color }}
            />
          </div>
          <div className="muted" style={{ marginTop: 6 }}>
            {s.label}
          </div>
        </div>

        <div className="field">
          <label className="field__label">确认主密码</label>
          <input
            className="field__input"
            type="password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void submit();
            }}
          />
          {mismatch && <div className="error">两次输入不一致</div>}
        </div>

        <div className="notice">
          下一步会生成一次性恢复码。它和主密码缺一不可：两者都丢失，数据将无法恢复。
        </div>

        {error && <div className="error">{error}</div>}

        <div className="footerbar" style={{ marginTop: 18 }}>
          <button
            className="btn btn--primary"
            disabled={!canSubmit || busy}
            onClick={() => void submit()}
          >
            {busy ? "正在派生密钥…" : "创建保险库"}
          </button>
        </div>
      </div>
    </div>
  );
}

function UnlockForm() {
  const unlock = useApp((s) => s.unlock);
  const recover = useApp((s) => s.recover);
  const busy = useApp((s) => s.busy);
  const error = useApp((s) => s.error);
  const clearError = useApp((s) => s.clearError);

  const [password, setPassword] = useState("");
  const [showRecovery, setShowRecovery] = useState(false);
  const [code, setCode] = useState("");

  async function submit() {
    if (busy) return;
    if (showRecovery) {
      if (code.trim().length < 20) return;
      if (await recover(code)) {
        setCode("");
        setShowRecovery(false);
      }
      return;
    }
    if (!password) return;
    if (await unlock(password)) setPassword("");
  }

  return (
    <div className="gate">
      <div className="gate__card">
        <img className="gate__mark" src={logoUrl} alt="" />
        <h1 className="gate__title">{showRecovery ? "用恢复码解锁" : "解锁 loki"}</h1>
        <p className="gate__sub">
          {showRecovery
            ? "输入创建保险库时保存的恢复码。解锁后可以设置新的主密码。"
            : "输入主密码以解开本机的保险库。"}
        </p>

        {showRecovery ? (
          <div className="field">
            <label className="field__label">恢复码</label>
            <textarea
              className="field__input"
              style={{
                height: 72,
                padding: 10,
                resize: "none",
                fontFamily: "ui-monospace, Menlo, monospace",
                fontSize: 13,
              }}
              autoFocus
              placeholder="XXXX XXXX XXXX XXXX XXXX XXXX XXXX XXXX"
              value={code}
              onChange={(e) => {
                setCode(e.target.value);
                clearError();
              }}
            />
            <div className="muted" style={{ marginTop: 6 }}>
              大小写、空格和连字符都不影响识别。
            </div>
          </div>
        ) : (
          <div className="field">
            <label className="field__label">主密码</label>
            <input
              className="field__input"
              type="password"
              autoFocus
              value={password}
              onChange={(e) => {
                setPassword(e.target.value);
                clearError();
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") void submit();
              }}
            />
          </div>
        )}

        {error && <div className="error">{error}</div>}

        <div className="footerbar" style={{ marginTop: 18, alignItems: "center" }}>
          <button
            className="btn btn--ghost"
            onClick={() => {
              setShowRecovery(!showRecovery);
              clearError();
            }}
          >
            {showRecovery ? "用主密码解锁" : "忘记密码？"}
          </button>
          <div className="spacer" />
          <button
            className="btn btn--primary"
            disabled={busy || (showRecovery ? code.trim().length < 20 : !password)}
            onClick={() => void submit()}
          >
            {busy ? "处理中…" : "解锁"}
          </button>
        </div>
      </div>
    </div>
  );
}
