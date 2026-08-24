import { Edit3, KeyRound, Plus, Save, Trash2, X } from "lucide-react";
import { FormEvent, ReactNode, useEffect, useState } from "react";
import type {
  ProxyInput,
  ProxyProfile,
  TerminalProfile,
  TerminalProfileInput,
  TerminalPalette,
  WorkspaceSnapshot,
} from "./types";
import { DEFAULT_CUSTOM_PALETTE, resolveTerminalTheme, TERMINAL_THEME_PRESETS } from "./terminalThemes";

interface SettingsDialogProps {
  snapshot: WorkspaceSnapshot;
  busy: boolean;
  error: string | null;
  onClose(): void;
  onSaveProxy(input: ProxyInput, existing?: ProxyProfile): Promise<void>;
  onDeleteProxy(value: ProxyProfile): Promise<void>;
  onConfigureProxyCredential(value: ProxyProfile): Promise<boolean>;
  proxyCredentialStatus(value: ProxyProfile): Promise<boolean>;
  onSaveTerminal(input: TerminalProfileInput, existing?: TerminalProfile): Promise<void>;
  onDeleteTerminal(value: TerminalProfile): Promise<void>;
}

export function SettingsDialog(props: SettingsDialogProps) {
  const [tab, setTab] = useState<"proxy" | "terminal">("proxy");
  const [editingProxy, setEditingProxy] = useState<ProxyProfile | null | undefined>(undefined);
  const [editingTerminal, setEditingTerminal] = useState<TerminalProfile | null | undefined>(undefined);
  const [proxyCredentials, setProxyCredentials] = useState<Record<string, boolean>>({});

  useEffect(() => {
    if (tab !== "proxy" || editingProxy !== undefined) return;
    let active = true;
    void Promise.all(props.snapshot.proxies.map(async (proxy) => [proxy.id, await props.proxyCredentialStatus(proxy)] as const))
      .then((entries) => { if (active) setProxyCredentials(Object.fromEntries(entries)); })
      .catch(() => undefined);
    return () => { active = false; };
  }, [editingProxy, props.snapshot.proxies, props.proxyCredentialStatus, tab]);

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
                  detail={(value) => `${value.kind}${value.username ? proxyCredentials[value.id] ? " · 凭据已配置" : " · 未配置凭据" : ""}`}
                  onAdd={() => setEditingProxy(null)}
                  onEdit={(value) => setEditingProxy(value)}
                  onDelete={props.onDeleteProxy}
                  extraAction={(value) => value.username && (value.kind === "socks5" || value.kind === "http_connect") ? <button className="icon-button" type="button" aria-label={`配置 ${value.name} 的代理凭据`} title="配置代理凭据" onClick={() => void props.onConfigureProxyCredential(value).then((configured) => { if (configured) setProxyCredentials((current) => ({ ...current, [value.id]: true })); }).catch(() => undefined)}><KeyRound size={15} /></button> : null}
                />
              ) : (
                <ProxyForm value={editingProxy ?? undefined} busy={props.busy} error={props.error} onCancel={() => setEditingProxy(undefined)} onSave={async (input) => { try { await props.onSaveProxy(input, editingProxy ?? undefined); setEditingProxy(undefined); } catch {} }} />
              )
            ) : editingTerminal === undefined ? (
              <ProfileList<TerminalProfile>
                title="终端配置"
                items={props.snapshot.terminalProfiles}
                label={(value) => value.name}
                detail={(value) => `${value.fontFamily} · ${value.fontSize}px · ${terminalThemeLabel(value.theme)}`}
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
  extraAction?(value: T): ReactNode;
}

function ProfileList<T>(props: ProfileListProps<T>) {
  return (
    <div className="profile-list">
      <div className="settings-section-title"><h3>{props.title}</h3><button className="icon-button" type="button" aria-label={`新建${props.title}`} title="新建" onClick={props.onAdd}><Plus size={17} /></button></div>
      {props.items.map((item, index) => (
        <div className="profile-row" key={index}>
          <div><strong>{props.label(item)}</strong><span>{props.detail(item)}</span></div>
          <div className="row-actions">{props.extraAction?.(item)}<button className="icon-button" type="button" aria-label={`编辑 ${props.label(item)}`} title="编辑" onClick={() => props.onEdit(item)}><Edit3 size={15} /></button><button className="icon-button danger" type="button" aria-label={`删除 ${props.label(item)}`} title="删除" onClick={() => void props.onDelete(item).catch(() => undefined)}><Trash2 size={15} /></button></div>
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
  const [dnsMode, setDnsMode] = useState<ProxyInput["dnsMode"]>(value?.dnsMode ?? "remote");
  const endpoint = kind === "socks5" || kind === "http_connect";
  const submit = (event: FormEvent) => {
    event.preventDefault();
    return onSave({ kind, name, host: endpoint ? host : "", port: endpoint ? port : 0, username: endpoint ? username : "", dnsMode });
  };
  return (
    <form className="entity-form" onSubmit={submit}>
      <div className="form-row"><label><span>名称</span><input autoFocus required maxLength={128} value={name} onChange={(event) => setName(event.target.value)} /></label><label><span>类型</span><select value={kind} onChange={(event) => setKind(event.target.value as ProxyInput["kind"])}><option value="direct">Direct</option><option value="socks5">SOCKS5</option><option value="http_connect">HTTP CONNECT</option><option value="system">系统代理</option></select></label></div>
      {endpoint && <><div className="form-row endpoint-row"><label><span>主机</span><input required value={host} onChange={(event) => setHost(event.target.value)} /></label><label><span>端口</span><input type="number" min={1} max={65535} value={port} onChange={(event) => setPort(Number(event.target.value))} /></label></div><div className="form-row"><label><span>用户名</span><input value={username} onChange={(event) => setUsername(event.target.value)} /></label><label><span>DNS 解析</span><select value={dnsMode} onChange={(event) => setDnsMode(event.target.value as ProxyInput["dnsMode"])}><option value="remote">代理端解析</option><option value="local">本地解析</option></select></label></div></>}
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
  const [palette, setPalette] = useState<TerminalPalette>(() => structuredClone(value?.palette ?? DEFAULT_CUSTOM_PALETTE));
  const [scrollback, setScrollback] = useState(value?.scrollback ?? 10000);
  const [encoding, setEncoding] = useState<TerminalProfileInput["encoding"]>(value?.encoding ?? "utf8");
  const [keymap, setKeymap] = useState<TerminalProfileInput["keymap"]>(value?.keymap ?? "xterm");
  const submit = (event: FormEvent) => {
    event.preventDefault();
    return onSave({ name, fontFamily, fontSize, theme, palette: theme === "custom" ? palette : null, scrollback, encoding, keymap });
  };
  const resolvedPalette = resolveTerminalTheme(theme, theme === "custom" ? palette : null);
  const updateColor = (key: keyof TerminalPalette, color: string) => setPalette((current) => ({ ...current, [key]: color }));
  return (
    <form className="entity-form" onSubmit={submit}>
      <label><span>名称</span><input autoFocus required value={name} onChange={(event) => setName(event.target.value)} /></label>
      <div className="form-row endpoint-row"><label><span>字体</span><input required value={fontFamily} onChange={(event) => setFontFamily(event.target.value)} /></label><label><span>字号</span><input type="number" min={8} max={32} step={0.5} value={fontSize} onChange={(event) => setFontSize(Number(event.target.value))} /></label></div>
      <div className="form-row"><label><span>主题</span><select value={theme} onChange={(event) => setTheme(event.target.value)}>{TERMINAL_THEME_PRESETS.map((preset) => <option key={preset.id} value={preset.id}>{preset.label}</option>)}<option value="custom">自定义</option></select></label><label><span>滚屏行数</span><input type="number" min={100} max={200000} step={100} value={scrollback} onChange={(event) => setScrollback(Number(event.target.value))} /></label></div>
      <div className="terminal-theme-preview" style={{ color: resolvedPalette.foreground, background: resolvedPalette.background, caretColor: resolvedPalette.cursor }} aria-label="终端主题预览"><span style={{ color: resolvedPalette.green }}>operator@server</span><span>:</span><span style={{ color: resolvedPalette.blue }}>~/workspace</span><span>$ </span><strong>printf</strong><span style={{ color: resolvedPalette.yellow }}> "FirstShell"</span><i style={{ background: resolvedPalette.selectionBackground }}> theme preview </i></div>
      {theme === "custom" && <fieldset className="terminal-palette-editor"><legend>自定义调色板</legend><div className="palette-grid">{TERMINAL_COLOR_FIELDS.map(([key, label]) => <label key={key}><span>{label}</span><input aria-label={label} type="color" value={palette[key]} onChange={(event) => updateColor(key, event.target.value)} /><code>{palette[key]}</code></label>)}</div></fieldset>}
      <div className="form-row"><label><span>编码</span><select value={encoding} onChange={(event) => setEncoding(event.target.value as TerminalProfileInput["encoding"])}><option value="utf8">UTF-8</option><option value="gb18030">GB18030</option><option value="big5">Big5</option></select></label><label><span>键位</span><select value={keymap} onChange={(event) => setKeymap(event.target.value as TerminalProfileInput["keymap"])}><option value="xterm">xterm</option><option value="windows">Windows</option><option value="emacs">Emacs</option></select></label></div>
      {error && <div className="inline-error" role="alert">{error}</div>}
      <footer className="modal-actions"><button type="button" onClick={onCancel}>返回</button><button className="primary-command" type="submit" disabled={busy}><Save size={16} /><span>保存</span></button></footer>
    </form>
  );
}

const TERMINAL_COLOR_FIELDS: readonly (readonly [keyof TerminalPalette, string])[] = [
  ["foreground", "前景"], ["background", "背景"], ["cursor", "光标"], ["cursorAccent", "光标反色"], ["selectionBackground", "选区"],
  ["black", "黑"], ["red", "红"], ["green", "绿"], ["yellow", "黄"], ["blue", "蓝"], ["magenta", "品红"], ["cyan", "青"], ["white", "白"],
  ["brightBlack", "亮黑"], ["brightRed", "亮红"], ["brightGreen", "亮绿"], ["brightYellow", "亮黄"], ["brightBlue", "亮蓝"], ["brightMagenta", "亮品红"], ["brightCyan", "亮青"], ["brightWhite", "亮白"],
];

function terminalThemeLabel(theme: string) {
  return theme === "custom" ? "自定义" : TERMINAL_THEME_PRESETS.find((preset) => preset.id === theme)?.label ?? theme;
}
