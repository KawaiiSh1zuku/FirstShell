import { Edit3, Plus, Save, Trash2, X } from "lucide-react";
import { FormEvent, useState } from "react";
import type {
  ProxyInput,
  ProxyProfile,
  TerminalProfile,
  TerminalProfileInput,
  WorkspaceSnapshot,
} from "./types";

interface SettingsDialogProps {
  snapshot: WorkspaceSnapshot;
  busy: boolean;
  error: string | null;
  onClose(): void;
  onSaveProxy(input: ProxyInput, existing?: ProxyProfile): Promise<void>;
  onDeleteProxy(value: ProxyProfile): Promise<void>;
  onSaveTerminal(input: TerminalProfileInput, existing?: TerminalProfile): Promise<void>;
  onDeleteTerminal(value: TerminalProfile): Promise<void>;
}

export function SettingsDialog(props: SettingsDialogProps) {
  const [tab, setTab] = useState<"proxy" | "terminal">("proxy");
  const [editingProxy, setEditingProxy] = useState<ProxyProfile | null | undefined>(undefined);
  const [editingTerminal, setEditingTerminal] = useState<TerminalProfile | null | undefined>(undefined);

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && props.onClose()}>
      <section className="modal settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <header className="modal-header">
          <h2 id="settings-title">工作区配置</h2>
          <button className="icon-button" type="button" aria-label="关闭" title="关闭" onClick={props.onClose}><X size={17} /></button>
        </header>
        <div className="settings-body">
          <nav className="settings-tabs" aria-label="配置类别">
            <button type="button" aria-current={tab === "proxy"} onClick={() => setTab("proxy")}>代理</button>
            <button type="button" aria-current={tab === "terminal"} onClick={() => setTab("terminal")}>终端</button>
          </nav>
          <div className="settings-content">
            {tab === "proxy" ? (
              editingProxy === undefined ? (
                <ProfileList<ProxyProfile>
                  title="代理配置"
                  items={props.snapshot.proxies}
                  label={(value) => value.name}
                  detail={(value) => value.kind}
                  onAdd={() => setEditingProxy(null)}
                  onEdit={(value) => setEditingProxy(value)}
                  onDelete={props.onDeleteProxy}
                />
              ) : (
                <ProxyForm value={editingProxy ?? undefined} busy={props.busy} error={props.error} onCancel={() => setEditingProxy(undefined)} onSave={async (input) => { try { await props.onSaveProxy(input, editingProxy ?? undefined); setEditingProxy(undefined); } catch {} }} />
              )
            ) : editingTerminal === undefined ? (
              <ProfileList<TerminalProfile>
                title="终端配置"
                items={props.snapshot.terminalProfiles}
                label={(value) => value.name}
                detail={(value) => `${value.fontFamily} · ${value.fontSize}px`}
                onAdd={() => setEditingTerminal(null)}
                onEdit={(value) => setEditingTerminal(value)}
                onDelete={props.onDeleteTerminal}
              />
            ) : (
              <TerminalForm value={editingTerminal ?? undefined} busy={props.busy} error={props.error} onCancel={() => setEditingTerminal(undefined)} onSave={async (input) => { try { await props.onSaveTerminal(input, editingTerminal ?? undefined); setEditingTerminal(undefined); } catch {} }} />
            )}
          </div>
        </div>
      </section>
    </div>
  );
}

interface ProfileListProps<T> {
  title: string;
  items: T[];
  label(value: T): string;
  detail(value: T): string;
  onAdd(): void;
  onEdit(value: T): void;
  onDelete(value: T): Promise<void>;
}

function ProfileList<T>(props: ProfileListProps<T>) {
  return (
    <div className="profile-list">
      <div className="settings-section-title"><h3>{props.title}</h3><button className="icon-button" type="button" aria-label={`新建${props.title}`} title="新建" onClick={props.onAdd}><Plus size={17} /></button></div>
      {props.items.map((item, index) => (
        <div className="profile-row" key={index}>
          <div><strong>{props.label(item)}</strong><span>{props.detail(item)}</span></div>
          <div className="row-actions"><button className="icon-button" type="button" aria-label={`编辑 ${props.label(item)}`} title="编辑" onClick={() => props.onEdit(item)}><Edit3 size={15} /></button><button className="icon-button danger" type="button" aria-label={`删除 ${props.label(item)}`} title="删除" onClick={() => void props.onDelete(item).catch(() => undefined)}><Trash2 size={15} /></button></div>
        </div>
      ))}
      {props.items.length === 0 && <div className="empty-list">暂无配置</div>}
    </div>
  );
}

function ProxyForm({ value, busy, error, onCancel, onSave }: { value?: ProxyProfile; busy: boolean; error: string | null; onCancel(): void; onSave(input: ProxyInput): Promise<void> }) {
  const [kind, setKind] = useState<ProxyInput["kind"]>(value?.kind ?? "socks5");
  const [name, setName] = useState(value?.name ?? "");
  const [host, setHost] = useState(value?.host ?? "");
  const [port, setPort] = useState(value?.port ?? 1080);
  const [username, setUsername] = useState(value?.username ?? "");
  const endpoint = kind === "socks5" || kind === "http_connect";
  const submit = (event: FormEvent) => {
    event.preventDefault();
    return onSave({ kind, name, host: endpoint ? host : "", port: endpoint ? port : 0, username: endpoint ? username : "" });
  };
  return (
    <form className="entity-form" onSubmit={submit}>
      <div className="form-row"><label><span>名称</span><input autoFocus required maxLength={128} value={name} onChange={(event) => setName(event.target.value)} /></label><label><span>类型</span><select value={kind} onChange={(event) => setKind(event.target.value as ProxyInput["kind"])}><option value="direct">Direct</option><option value="socks5">SOCKS5</option><option value="http_connect">HTTP CONNECT</option><option value="system">系统代理</option></select></label></div>
      {endpoint && <><div className="form-row endpoint-row"><label><span>主机</span><input required value={host} onChange={(event) => setHost(event.target.value)} /></label><label><span>端口</span><input type="number" min={1} max={65535} value={port} onChange={(event) => setPort(Number(event.target.value))} /></label></div><label><span>用户名</span><input value={username} onChange={(event) => setUsername(event.target.value)} /></label></>}
      {error && <div className="inline-error" role="alert">{error}</div>}
      <footer className="modal-actions"><button type="button" onClick={onCancel}>返回</button><button className="primary-command" type="submit" disabled={busy}><Save size={16} /><span>保存</span></button></footer>
    </form>
  );
}

function TerminalForm({ value, busy, error, onCancel, onSave }: { value?: TerminalProfile; busy: boolean; error: string | null; onCancel(): void; onSave(input: TerminalProfileInput): Promise<void> }) {
  const [name, setName] = useState(value?.name ?? "");
  const [fontFamily, setFontFamily] = useState(value?.fontFamily ?? "Cascadia Mono");
  const [fontSize, setFontSize] = useState(value?.fontSize ?? 14);
  const [theme, setTheme] = useState(value?.theme ?? "firstshell-dark");
  const [scrollback, setScrollback] = useState(value?.scrollback ?? 10000);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    return onSave({ name, fontFamily, fontSize, theme, scrollback });
  };
  return (
    <form className="entity-form" onSubmit={submit}>
      <label><span>名称</span><input autoFocus required value={name} onChange={(event) => setName(event.target.value)} /></label>
      <div className="form-row endpoint-row"><label><span>字体</span><input required value={fontFamily} onChange={(event) => setFontFamily(event.target.value)} /></label><label><span>字号</span><input type="number" min={8} max={32} step={0.5} value={fontSize} onChange={(event) => setFontSize(Number(event.target.value))} /></label></div>
      <div className="form-row"><label><span>主题</span><select value={theme} onChange={(event) => setTheme(event.target.value)}><option value="firstshell-dark">FirstShell Dark</option><option value="firstshell-light">FirstShell Light</option><option value="high-contrast">High Contrast</option></select></label><label><span>滚屏行数</span><input type="number" min={100} max={200000} step={100} value={scrollback} onChange={(event) => setScrollback(Number(event.target.value))} /></label></div>
      {error && <div className="inline-error" role="alert">{error}</div>}
      <footer className="modal-actions"><button type="button" onClick={onCancel}>返回</button><button className="primary-command" type="submit" disabled={busy}><Save size={16} /><span>保存</span></button></footer>
    </form>
  );
}
