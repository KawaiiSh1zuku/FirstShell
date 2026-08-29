import { KeyRound, Save, X } from "lucide-react";
import { FormEvent, useEffect, useState } from "react";
import { createPortal } from "react-dom";

export function CredentialDialog({ title, description, label, persistent, onClose, onSubmit }: {
  title: string;
  description: string;
  label: string;
  persistent: boolean;
  onClose(): void;
  onSubmit(secret: string): Promise<void>;
}) {
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [busy, onClose]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const submitted = secret;
    setSecret("");
    setBusy(true);
    setError(null);
    try {
      await onSubmit(submitted);
      onClose();
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  };

  return createPortal(
    <div className="modal-backdrop credential-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && !busy && onClose()}>
      <section className="modal credential-modal" role="dialog" aria-modal="true" aria-labelledby="credential-dialog-title">
        <header className="modal-header"><div className="credential-title"><KeyRound size={17} /><h2 id="credential-dialog-title">{title}</h2></div><button className="icon-button" type="button" aria-label="关闭" title="关闭" disabled={busy} onClick={onClose}><X size={17} /></button></header>
        <form className="credential-form" onSubmit={(event) => void submit(event)}>
          <p>{description}</p>
          <label><span>{label}</span><input autoFocus required type="password" autoComplete="new-password" maxLength={4096} value={secret} onChange={(event) => setSecret(event.target.value)} /></label>
          <small>{persistent ? "凭据将保存到操作系统凭据库，不写入工作区数据库。" : "凭据仅用于本次连接，不会保存。"}</small>
          {error && <div className="inline-error" role="alert">{error}</div>}
          <footer className="modal-actions"><button type="button" disabled={busy} onClick={onClose}>取消</button><button className="primary-command" type="submit" disabled={busy || secret.length === 0}><Save size={16} /><span>{busy ? "保存中" : persistent ? "保存并重连" : "连接"}</span></button></footer>
        </form>
      </section>
    </div>,
    document.body,
  );
}

function message(reason: unknown) {
  return reason instanceof Error ? reason.message : typeof reason === "object" && reason && "message" in reason ? String(reason.message) : "凭据保存失败";
}
