import { Play, X } from "lucide-react";
import { FormEvent, useState } from "react";
import type { QuickConnectInput } from "./types";

export function QuickConnectDialog({ busy, error, onClose, onConnect }: {
  busy: boolean;
  error: string | null;
  onClose(): void;
  onConnect(input: QuickConnectInput): Promise<void>;
}) {
  const [host, setHost] = useState("");
  const [port, setPort] = useState(22);
  const [username, setUsername] = useState("");
  const [authentication, setAuthentication] = useState<QuickConnectInput["authentication"]>("agent");
  const [privateKeyPath, setPrivateKeyPath] = useState("");
  const [allowLegacyAlgorithms, setAllowLegacyAlgorithms] = useState(false);
  const [opensshFallback, setOpensshFallback] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    await onConnect({
      host: host.trim(),
      port,
      username: username.trim(),
      authentication,
      privateKeyPath: authentication === "private_key" ? privateKeyPath.trim() : "",
      allowLegacyAlgorithms,
      opensshFallback: authentication === "agent" && opensshFallback,
    });
  };

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section className="modal quick-connect-modal" role="dialog" aria-modal="true" aria-labelledby="quick-connect-title">
        <header className="modal-header"><h2 id="quick-connect-title">快速连接</h2><button className="icon-button" type="button" aria-label="关闭" title="关闭" onClick={onClose}><X size={17} /></button></header>
        <form className="entity-form" onSubmit={(event) => void submit(event)}>
          <div className="form-row endpoint-row"><label><span>主机</span><input autoFocus required maxLength={253} value={host} onChange={(event) => setHost(event.target.value)} /></label><label><span>端口</span><input type="number" min={1} max={65535} required value={port} onChange={(event) => setPort(Number(event.target.value))} /></label></div>
          <label><span>用户名</span><input required maxLength={128} value={username} onChange={(event) => setUsername(event.target.value)} /></label>
          <div className="form-row"><label><span>认证</span><select value={authentication} onChange={(event) => setAuthentication(event.target.value as QuickConnectInput["authentication"])}><option value="agent">SSH Agent</option><option value="password">密码</option><option value="private_key">私钥</option><option value="keyboard_interactive">Keyboard Interactive</option></select></label>{authentication === "private_key" ? <label><span>私钥路径</span><input required maxLength={4096} value={privateKeyPath} onChange={(event) => setPrivateKeyPath(event.target.value)} /></label> : <span />}</div>
          <div className="binary-options"><label><input type="checkbox" checked={allowLegacyAlgorithms} onChange={(event) => setAllowLegacyAlgorithms(event.target.checked)} /><span>允许兼容算法</span></label><label><input type="checkbox" checked={authentication === "agent" && opensshFallback} disabled={authentication !== "agent"} onChange={(event) => setOpensshFallback(event.target.checked)} /><span>允许 OpenSSH 回退（仅 Agent）</span></label></div>
          {error && <div className="inline-error" role="alert">{error}</div>}
          <footer className="modal-actions"><button type="button" onClick={onClose}>取消</button><button className="primary-command" type="submit" disabled={busy}><Play size={16} /><span>{busy ? "连接中" : "连接"}</span></button></footer>
        </form>
      </section>
    </div>
  );
}
