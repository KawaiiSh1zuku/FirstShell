import { FileCode2, History, RadioTower, Search, Send, Tags, Wrench } from "lucide-react";
import { FormEvent, useEffect, useMemo, useState } from "react";
import type { WorkspaceBridge } from "./bridge";
import type {
  CommandHistoryEntry,
  ConnectionProfile,
  ReadyTerminalSession,
  Snippet,
  SnippetInput,
  SnippetParameter,
} from "./types";

type ToolTab = "snippets" | "history" | "broadcast";

interface ToolsWorkspaceProps {
  bridge: WorkspaceBridge;
  connections: ConnectionProfile[];
  snippets: Snippet[];
  activeConnectionId: string | null;
  readySessions: ReadyTerminalSession[];
  onInsert(connectionId: string, text: string): void;
  onRefresh(): Promise<void>;
  onActivity(message: string): void;
}

export function ToolsWorkspace(props: ToolsWorkspaceProps) {
  const [tab, setTab] = useState<ToolTab>("snippets");
  return <div className="tools-workspace">
    <nav className="tools-tabs" aria-label="效率工具">
      <ToolTabButton value="snippets" active={tab} onSelect={setTab} icon={<FileCode2 size={13} />}>片段</ToolTabButton>
      <ToolTabButton value="history" active={tab} onSelect={setTab} icon={<History size={13} />}>历史</ToolTabButton>
      <ToolTabButton value="broadcast" active={tab} onSelect={setTab} icon={<RadioTower size={13} />}>广播</ToolTabButton>
    </nav>
    {tab === "snippets" && <SnippetTool {...props} />}
    {tab === "history" && <HistoryTool {...props} />}
    {tab === "broadcast" && <BroadcastTool {...props} />}
  </div>;
}

function ToolTabButton({ value, active, onSelect, icon, children }: { value: ToolTab; active: ToolTab; onSelect(value: ToolTab): void; icon: React.ReactNode; children: React.ReactNode }) {
  return <button type="button" aria-current={active === value} onClick={() => onSelect(value)}>{icon}<span>{children}</span></button>;
}

function SnippetTool(props: ToolsWorkspaceProps) {
  const [selectedId, setSelectedId] = useState(props.snippets[0]?.id ?? "");
  const selected = props.snippets.find((value) => value.id === selectedId);
  const [draft, setDraft] = useState<SnippetInput>(() => snippetInput(selected));
  const [values, setValues] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => { setDraft(snippetInput(selected)); setValues(Object.fromEntries((selected?.parameters ?? []).map((value) => [value.name, value.defaultValue]))); }, [selected]);

  const save = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null);
    try {
      const saved = selected ? await props.bridge.updateSnippet(selected.id, selected.revision, draft) : await props.bridge.createSnippet(draft);
      await props.onRefresh(); setSelectedId(saved.id); props.onActivity(selected ? "命令片段已更新" : "命令片段已创建");
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const remove = async () => {
    if (!selected || !window.confirm(`删除片段“${selected.name}”？`)) return;
    setBusy(true); setError(null);
    try { await props.bridge.deleteSnippet(selected.id, selected.revision); await props.onRefresh(); setSelectedId(""); props.onActivity("命令片段已删除"); }
    catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const insert = async () => {
    if (!selected || !props.activeConnectionId) return;
    setBusy(true); setError(null);
    try { props.onInsert(props.activeConnectionId, await props.bridge.renderSnippet(selected.id, values)); props.onActivity("片段已插入活动终端（未执行）"); }
    catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  return <section className="tool-pane snippet-tool">
    <div className="snippet-list"><button type="button" className={!selected ? "selected" : ""} onClick={() => setSelectedId("")}>＋ 新片段</button>{props.snippets.map((value) => <button type="button" className={value.id === selectedId ? "selected" : ""} key={value.id} onClick={() => setSelectedId(value.id)}><strong>{value.name}</strong><small>{value.tags.join(" · ") || "无标签"}</small></button>)}</div>
    <form className="snippet-editor" onSubmit={(event) => void save(event)}>
      <label className="tool-field"><span><FileCode2 size={12} />片段名称</span><input aria-label="片段名称" value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value })} placeholder="例如：查看服务日志" /></label>
      <label className="tool-field"><span><Tags size={12} />标签</span><input aria-label="片段标签" value={draft.tags.join(", ")} onChange={(event) => setDraft({ ...draft, tags: event.target.value.split(",").map((value) => value.trim()).filter(Boolean) })} placeholder="systemd, logs" /></label>
      <label className="tool-field"><span><Wrench size={12} />命令模板</span><textarea aria-label="片段模板" rows={3} value={draft.template} onChange={(event) => setDraft({ ...draft, template: event.target.value })} placeholder="journalctl -u {{service}}" /></label>
      <div className="parameter-list"><div><strong>类型化参数</strong><button type="button" onClick={() => setDraft({ ...draft, parameters: [...draft.parameters, emptyParameter(draft.parameters.length + 1)] })}>添加</button></div>{draft.parameters.map((parameter, index) => <ParameterEditor key={index} value={parameter} onChange={(value) => setDraft({ ...draft, parameters: draft.parameters.map((item, itemIndex) => itemIndex === index ? value : item) })} onRemove={() => setDraft({ ...draft, parameters: draft.parameters.filter((_, itemIndex) => itemIndex !== index) })} />)}</div>
      {error && <div className="tool-error" role="alert">{error}</div>}
      <div className="tool-actions"><button className="primary-command" type="submit" disabled={busy}>{selected ? "保存" : "创建"}</button>{selected && <button type="button" disabled={busy} onClick={() => void remove()}>删除</button>}</div>
    </form>
    {selected && <div className="snippet-runner"><strong>插入参数</strong>{selected.parameters.map((parameter) => <label key={parameter.name}><span>{parameter.label}</span>{parameter.kind === "choice" ? <select value={values[parameter.name] ?? ""} onChange={(event) => setValues({ ...values, [parameter.name]: event.target.value })}><option value="">选择</option>{parameter.choices.map((choice) => <option key={choice}>{choice}</option>)}</select> : <input type={parameter.kind === "integer" ? "number" : "text"} value={values[parameter.name] ?? ""} onChange={(event) => setValues({ ...values, [parameter.name]: event.target.value })} />}</label>)}<button className="primary-command" type="button" disabled={!props.activeConnectionId || busy} onClick={() => void insert()}>插入活动终端</button><small>仅插入文本，不发送回车。</small></div>}
  </section>;
}

function ParameterEditor({ value, onChange, onRemove }: { value: SnippetParameter; onChange(value: SnippetParameter): void; onRemove(): void }) {
  return <div className="parameter-row"><input aria-label="参数名" value={value.name} onChange={(event) => onChange({ ...value, name: event.target.value })} placeholder="name" /><input aria-label="参数标签" value={value.label} onChange={(event) => onChange({ ...value, label: event.target.value })} placeholder="标签" /><select aria-label="参数类型" value={value.kind} onChange={(event) => onChange({ ...value, kind: event.target.value as SnippetParameter["kind"] })}><option value="text">文本</option><option value="integer">整数</option><option value="choice">选项</option><option value="host">主机</option><option value="path">路径</option></select><input aria-label="参数默认值" value={value.defaultValue} onChange={(event) => onChange({ ...value, defaultValue: event.target.value })} placeholder="默认值" />{value.kind === "choice" && <input aria-label="参数选项" value={value.choices.join(", ")} onChange={(event) => onChange({ ...value, choices: event.target.value.split(",").map((item) => item.trim()).filter(Boolean) })} placeholder="选项，以逗号分隔" />}<label><input type="checkbox" checked={value.required} onChange={(event) => onChange({ ...value, required: event.target.checked })} />必填</label><button type="button" aria-label={`删除参数 ${value.name || "未命名"}`} onClick={onRemove}>×</button></div>;
}

function HistoryTool(props: ToolsWorkspaceProps) {
  const [history, setHistory] = useState<CommandHistoryEntry[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const connectionId = props.activeConnectionId;
  const load = () => props.bridge.commandHistory(connectionId, 200).then(setHistory).catch((reason) => setError(message(reason)));
  useEffect(() => { void load(); }, [connectionId, props.bridge]);
  const filtered = history.filter((value) => `${value.command} ${value.path ?? ""}`.toLowerCase().includes(query.toLowerCase()));
  return <section className="tool-pane history-tool"><label className="tool-field tool-search"><span><Search size={12} />命令与路径</span><input aria-label="搜索命令历史" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索历史记录" /></label>{error && <div className="tool-error">{error}</div>}<div className="history-list">{filtered.map((value) => <button type="button" key={`${value.connectionId}-${value.recordedAtMs}-${value.command}`} onClick={() => props.onInsert(value.connectionId, value.command)}><code>{value.command}</code><small>{value.path ? `路径 ${value.path} · ` : ""}{new Date(value.recordedAtMs).toLocaleString()}</small></button>)}{!filtered.length && <div className="empty-list">暂无历史</div>}</div><div className="tool-actions"><button type="button" onClick={() => void load()}>刷新</button><button type="button" onClick={() => void props.bridge.clearCommandHistory().then(() => { setHistory([]); props.onActivity("命令历史已清除"); }).catch((reason) => setError(message(reason)))}>清除全部</button></div></section>;
}

function BroadcastTool(props: ToolsWorkspaceProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const [command, setCommand] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const available = useMemo(() => new Set(props.readySessions.map((value) => value.sessionId)), [props.readySessions]);
  useEffect(() => setSelected((current) => current.filter((id) => available.has(id))), [available]);
  const submit = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null);
    try { await props.bridge.broadcastTerminal(selected, command, confirmed); setConfirmed(false); props.onActivity(`命令已原子广播到 ${selected.length} 个终端`); }
    catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };
  return <section className="tool-pane broadcast-tool"><div className="broadcast-targets">{props.readySessions.map((session) => { const connection = props.connections.find((value) => value.id === session.connectionId); return <label key={session.sessionId}><input type="checkbox" checked={selected.includes(session.sessionId)} onChange={(event) => setSelected(event.target.checked ? [...selected, session.sessionId] : selected.filter((id) => id !== session.sessionId))} /><span>{connection?.name ?? session.connectionId}</span><code>{session.sessionId.slice(0, 8)}</code></label>; })}{!props.readySessions.length && <div className="empty-list">暂无已就绪终端</div>}</div><form onSubmit={(event) => void submit(event)}><label className="tool-field"><span><Send size={12} />广播命令</span><textarea aria-label="广播命令" rows={3} value={command} onChange={(event) => { setCommand(event.target.value); setConfirmed(false); }} placeholder="输入一条单行命令" /></label><label className="broadcast-confirm"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />我确认向所选终端执行此命令</label>{error && <div className="tool-error">{error}</div>}<div className="tool-actions"><button className="primary-command" type="submit" disabled={busy || !confirmed || !command.trim() || selected.length === 0}>广播执行</button><button className="danger-command" type="button" disabled={busy || selected.length === 0} onClick={() => void props.bridge.stopBroadcast(selected).then(() => props.onActivity("已向所选终端发送 Ctrl-C")).catch((reason) => setError(message(reason)))}>紧急停止</button></div></form></section>;
}

function snippetInput(value?: Snippet): SnippetInput {
  return value ? { name: value.name, tags: [...value.tags], template: value.template, parameters: value.parameters.map((parameter) => ({ ...parameter, choices: [...parameter.choices] })) } : { name: "", tags: [], template: "", parameters: [] };
}

function emptyParameter(index: number): SnippetParameter {
  return { name: `value${index}`, label: `参数 ${index}`, kind: "text", required: true, defaultValue: "", choices: [] };
}

function message(reason: unknown) { return reason instanceof Error ? reason.message : typeof reason === "object" && reason && "message" in reason ? String((reason as { message: unknown }).message) : "操作失败"; }
