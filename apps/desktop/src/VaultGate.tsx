import { KeyRound, LockKeyhole, ShieldCheck } from "lucide-react";
import { FormEvent, useState } from "react";
import type { VaultStatus } from "./types";

interface VaultGateProps {
  status: VaultStatus;
  busy: boolean;
  error: string | null;
  onInitialize(password: string): Promise<void>;
  onUnlock(): Promise<void>;
  onRecover(password: string): Promise<void>;
}

export function VaultGate({
  status,
  busy,
  error,
  onInitialize,
  onUnlock,
  onRecover,
}: VaultGateProps) {
  const [mode, setMode] = useState<"unlock" | "recover">("unlock");
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!status.initialized) {
      if (password !== confirmation) return;
      await onInitialize(password);
      return;
    }
    if (mode === "recover") await onRecover(password);
    else await onUnlock();
  };

  const mismatch = !status.initialized && confirmation.length > 0 && password !== confirmation;

  return (
    <main className="vault-gate">
      <section className="vault-panel" aria-labelledby="vault-title">
        <div className="brand-mark" aria-hidden="true"><span>&gt;_</span></div>
        <div className="vault-heading">
          <h1 id="vault-title">FirstShell</h1>
          <p>{status.initialized ? "本地仓库已锁定" : "创建本地安全仓库"}</p>
        </div>

        {status.initialized && (
          <div className="segmented" aria-label="解锁方式">
            <button type="button" aria-pressed={mode === "unlock"} onClick={() => setMode("unlock")}>系统密钥</button>
            <button type="button" aria-pressed={mode === "recover"} onClick={() => setMode("recover")}>恢复口令</button>
          </div>
        )}

        <form className="vault-form" onSubmit={submit}>
          {!status.initialized && (
            <div className="security-row">
              <ShieldCheck size={18} aria-hidden="true" />
              <span>恢复口令为必填项</span>
            </div>
          )}

          {(!status.initialized || mode === "recover") && (
            <label>
              <span>恢复口令</span>
              <input
                autoFocus
                type="password"
                autoComplete="off"
                minLength={12}
                maxLength={1024}
                required
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </label>
          )}

          {!status.initialized && (
            <label>
              <span>确认恢复口令</span>
              <input
                type="password"
                autoComplete="off"
                minLength={12}
                maxLength={1024}
                required
                aria-invalid={mismatch}
                value={confirmation}
                onChange={(event) => setConfirmation(event.target.value)}
              />
              {mismatch && <small className="field-error">两次输入不一致</small>}
            </label>
          )}

          {error && <div className="inline-error" role="alert">{error}</div>}

          <button className="primary-command" type="submit" disabled={busy || mismatch}>
            {status.initialized ? <LockKeyhole size={17} /> : <KeyRound size={17} />}
            <span>{busy ? "处理中" : status.initialized ? "解锁" : "创建仓库"}</span>
          </button>
        </form>
      </section>
    </main>
  );
}
