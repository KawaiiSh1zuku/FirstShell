import { Activity, ArrowDownToLine, ArrowUpFromLine, ChevronDown, FolderPlus, KeyRound, Network, Pause, Play, RefreshCw, RotateCcw, Square, Trash2 } from "lucide-react";
import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import type { WorkspaceBridge } from "./bridge";
import type { ConnectionProfile, DesktopTunnelSnapshot, HostKeyEvidence, RemoteEntry, SftpWorkspaceSnapshot, TransferOptions, TransferSnapshot, TransferStatus, TunnelKind } from "./types";

interface OperationsWorkspaceProps {
  bridge: WorkspaceBridge;
  connections: ConnectionProfile[];
  activeConnectionId: string | null;
  activities: { id: number; message: string; time: string }[];
  onActivity(message: string): void;
  onCollapse(): void;
}

type BottomTab = "sftp" | "transfers" | "tunnels" | "activity";

const defaultTransferOptions: TransferOptions = { conflict: "resume", verify: "size" };

export function OperationsWorkspace(props: OperationsWorkspaceProps) {
  const sshConnections = useMemo(() => props.connections.filter((value) => value.kind === "ssh" && !value.temporary), [props.connections]);
  const [tab, setTab] = useState<BottomTab>("sftp");
  const [connectionId, setConnectionId] = useState(() => sshConnections.find((value) => value.id === props.activeConnectionId)?.id ?? sshConnections[0]?.id ?? "");
  const [root, setRoot] = useState(".");
  const [workspace, setWorkspace] = useState<SftpWorkspaceSnapshot | null>(null);
  const [path, setPath] = useState("");
  const [entries, setEntries] = useState<RemoteEntry[]>([]);
  const [selected, setSelected] = useState<RemoteEntry | null>(null);
  const [transfers, setTransfers] = useState<TransferSnapshot[]>([]);
  const [tunnels, setTunnels] = useState<DesktopTunnelSnapshot[]>([]);
  const [options, setOptions] = useState(defaultTransferOptions);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!workspace && props.activeConnectionId && sshConnections.some((value) => value.id === props.activeConnectionId)) setConnectionId(props.activeConnectionId);
  }, [props.activeConnectionId, sshConnections, workspace]);

  useEffect(() => {
    void Promise.all([props.bridge.listTransfers(), props.bridge.listTunnels()])
      .then(([nextTransfers, nextTunnels]) => { setTransfers(nextTransfers); setTunnels(nextTunnels); })
      .catch(() => undefined);
  }, [props.bridge]);

  const updateTransfer = useCallback((value: TransferSnapshot) => {
    setTransfers((current) => current.some((item) => item.id === value.id)
      ? current.map((item) => item.id === value.id ? value : item)
      : [value, ...current]);
  }, []);

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;
    void props.bridge.onSftpUploadDrop((drop) => {
      void (async () => {
        if (!active) return;
        if (drop.error) { setError(drop.error); return; }
        if (!drop.batchId || !workspace || workspace.state !== "ready") {
          setError("请先连接 SFTP 工作区，再将本地文件拖入窗口");
          return;
        }
        try {
          const queued = await props.bridge.queueSftpDrop(workspace.id, drop.batchId, path, options, updateTransfer);
          if (!active) return;
          setTransfers((current) => mergeTransfers(current, queued));
          if (queued.length) {
            setTab("transfers");
            props.onActivity(`${queued.length} 个拖放上传任务已加入队列`);
          }
        } catch (reason) {
          if (active) setError(message(reason));
        }
      })();
    }).then((unlisten) => {
      if (active) dispose = unlisten;
      else unlisten();
    });
    return () => { active = false; dispose?.(); };
  }, [options, path, props.bridge, props.onActivity, updateTransfer, workspace]);

  const loadDirectory = useCallback(async (nextPath: string, target = workspace) => {
    if (!target || target.state !== "ready") return;
    setBusy(true); setError(null);
    try {
      setEntries(await props.bridge.listSftp(target.id, nextPath));
      setPath(nextPath); setSelected(null);
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  }, [props.bridge, workspace]);

  const openSftp = async () => {
    if (!connectionId) return;
    setBusy(true); setError(null);
    try {
      const next = await props.bridge.openSftp(connectionId, root);
      setWorkspace(next);
      if (next.state === "ready") await loadDirectory("", next);
      props.onActivity(next.state === "ready" ? "SFTP 工作区已连接" : "SFTP 等待主机密钥确认");
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const closeSftp = async () => {
    if (!workspace) return;
    try { await props.bridge.closeSftp(workspace.id); } catch {}
    setWorkspace(null); setEntries([]); setPath(""); setSelected(null);
  };

  const approveSftp = async () => {
    if (!workspace?.hostKey) return;
    const evidence = workspace.hostKey;
    setBusy(true); setError(null);
    try {
      const next = await props.bridge.approveSftpHostKey(workspace.id, evidence.fingerprint, evidence.previousFingerprint);
      setWorkspace(next);
      if (next.state === "ready") await loadDirectory("", next);
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const mutateEntry = async (kind: "mkdir" | "rename" | "delete" | "chmod") => {
    if (!workspace || workspace.state !== "ready") return;
    try {
      if (kind === "mkdir") {
        const name = window.prompt("新目录名称");
        if (!name) return;
        await props.bridge.createSftpDirectory(workspace.id, path, name);
      } else if (kind === "rename" && selected) {
        const name = window.prompt("新名称", selected.name);
        if (!name || name === selected.name) return;
        await props.bridge.renameSftpEntry(workspace.id, selected.path, name);
      } else if (kind === "delete" && selected) {
        if (!window.confirm(`删除远程项目“${selected.name}”？此操作不可撤销。`)) return;
        await props.bridge.deleteSftpEntry(workspace.id, selected.path);
      } else if (kind === "chmod" && selected) {
        const raw = window.prompt("POSIX 权限（八进制）", ((selected.permissions ?? 0o644) & 0o7777).toString(8));
        if (!raw || !/^[0-7]{3,4}$/.test(raw)) return;
        await props.bridge.chmodSftpEntry(workspace.id, selected.path, Number.parseInt(raw, 8));
      }
      await loadDirectory(path);
    } catch (reason) { setError(message(reason)); }
  };

  const upload = async () => {
    if (!workspace || workspace.state !== "ready") return;
    try {
      const queued = await props.bridge.chooseSftpUploads(workspace.id, path, options, updateTransfer);
      setTransfers((current) => mergeTransfers(current, queued));
      if (queued.length) { setTab("transfers"); props.onActivity(`${queued.length} 个上传任务已加入队列`); }
    } catch (reason) { setError(message(reason)); }
  };

  const download = async () => {
    if (!workspace || workspace.state !== "ready" || selected?.kind !== "file") return;
    try {
      const queued = await props.bridge.chooseSftpDownload(workspace.id, selected.path, options, updateTransfer);
      if (queued) { setTransfers((current) => mergeTransfers(current, [queued])); setTab("transfers"); props.onActivity("下载任务已加入队列"); }
    } catch (reason) { setError(message(reason)); }
  };

  return (
    <section className="operations-panel" aria-label="文件与网络工具">
      <header className="operations-header">
        <nav aria-label="底部工具">
          <Tab value="sftp" active={tab} onSelect={setTab}>SFTP</Tab>
          <Tab value="transfers" active={tab} onSelect={setTab}>传输 {transfers.filter((value) => !isTerminal(value)).length || ""}</Tab>
          <Tab value="tunnels" active={tab} onSelect={setTab}>隧道 {tunnels.filter((value) => value.state === "running").length || ""}</Tab>
          <Tab value="activity" active={tab} onSelect={setTab}>活动</Tab>
        </nav>
        <button className="icon-button" type="button" aria-label="收起底部面板" title="收起" onClick={props.onCollapse}><ChevronDown size={15} /></button>
      </header>
      {error && <div className="operations-error" role="alert">{error}</div>}
      <div className="operations-content">
        {tab === "sftp" && <SftpView connections={sshConnections} connectionId={connectionId} onConnectionId={setConnectionId} root={root} onRoot={setRoot} workspace={workspace} path={path} entries={entries} selected={selected} busy={busy} options={options} onOptions={setOptions} onOpen={() => void openSftp()} onClose={() => void closeSftp()} onApprove={() => void approveSftp()} onOpenPath={(value) => void loadDirectory(value)} onSelect={setSelected} onRefresh={() => void loadDirectory(path)} onMutate={(value) => void mutateEntry(value)} onUpload={() => void upload()} onDownload={() => void download()} />}
        {tab === "transfers" && <TransferView transfers={transfers} onPause={async (id) => { await props.bridge.pauseTransfer(id); setTransfers(await props.bridge.listTransfers()); }} onCancel={async (id) => { await props.bridge.cancelTransfer(id); setTransfers(await props.bridge.listTransfers()); }} onRetry={async (id) => { const value = await props.bridge.retryTransfer(id, updateTransfer); setTransfers((current) => mergeTransfers(current, [value])); }} onClear={async () => { await props.bridge.clearFinishedTransfers(); setTransfers(await props.bridge.listTransfers()); }} />}
        {tab === "tunnels" && <TunnelView bridge={props.bridge} connections={sshConnections} defaultConnectionId={connectionId} tunnels={tunnels} onTunnels={setTunnels} onError={setError} onActivity={props.onActivity} />}
        {tab === "activity" && <div className="activity-list operations-activity">{props.activities.map((item) => <div key={item.id}><time>{item.time}</time><span>{item.message}</span></div>)}{props.activities.length === 0 && <div className="empty-list">暂无活动</div>}</div>}
      </div>
    </section>
  );
}

function Tab({ value, active, onSelect, children }: { value: BottomTab; active: BottomTab; onSelect(value: BottomTab): void; children: React.ReactNode }) {
  return <button type="button" aria-current={active === value} onClick={() => onSelect(value)}>{children}</button>;
}

function SftpView(props: { connections: ConnectionProfile[]; connectionId: string; onConnectionId(value: string): void; root: string; onRoot(value: string): void; workspace: SftpWorkspaceSnapshot | null; path: string; entries: RemoteEntry[]; selected: RemoteEntry | null; busy: boolean; options: TransferOptions; onOptions(value: TransferOptions): void; onOpen(): void; onClose(): void; onApprove(): void; onOpenPath(value: string): void; onSelect(value: RemoteEntry): void; onRefresh(): void; onMutate(value: "mkdir" | "rename" | "delete" | "chmod"): void; onUpload(): void; onDownload(): void }) {
  if (!props.workspace) return <div className="sftp-connect"><select aria-label="SFTP 连接" value={props.connectionId} onChange={(event) => props.onConnectionId(event.target.value)}><option value="">选择 SSH 连接</option>{props.connections.map((value) => <option key={value.id} value={value.id}>{value.name} · {value.host}</option>)}</select><input aria-label="远程根目录" value={props.root} onChange={(event) => props.onRoot(event.target.value)} placeholder="远程根目录" /><button className="primary-command" type="button" disabled={!props.connectionId || props.busy} onClick={props.onOpen}>连接 SFTP</button></div>;
  if (props.workspace.state === "host_key_required" && props.workspace.hostKey) return <HostKeyNotice evidence={props.workspace.hostKey} onApprove={props.onApprove} busy={props.busy} />;
  const crumbs = props.path ? props.path.split('/') : [];
  return <div className="sftp-browser"><div className="sftp-toolbar"><div className="breadcrumbs"><button type="button" onClick={() => props.onOpenPath("")}>{props.workspace.root}</button>{crumbs.map((value, index) => <button type="button" key={`${value}-${index}`} onClick={() => props.onOpenPath(crumbs.slice(0, index + 1).join('/'))}>/{value}</button>)}</div><div><button className="icon-button" type="button" aria-label="刷新 SFTP" title="刷新" onClick={props.onRefresh}><RefreshCw size={14} /></button><button className="icon-button" type="button" aria-label="新建远程目录" title="新建目录" onClick={() => props.onMutate("mkdir")}><FolderPlus size={14} /></button><button type="button" onClick={props.onClose}>关闭</button></div></div><div className="sftp-actions"><select aria-label="冲突策略" value={props.options.conflict} onChange={(event) => props.onOptions({ ...props.options, conflict: event.target.value as TransferOptions["conflict"] })}><option value="deny">拒绝覆盖</option><option value="replace">替换</option><option value="resume">续传</option></select><select aria-label="校验方式" value={props.options.verify} onChange={(event) => props.onOptions({ ...props.options, verify: event.target.value as TransferOptions["verify"] })}><option value="size">大小校验</option><option value="sha256">SHA-256</option></select><button type="button" onClick={props.onUpload}><ArrowUpFromLine size={14} />多文件上传</button><button type="button" disabled={props.selected?.kind !== "file"} onClick={props.onDownload}><ArrowDownToLine size={14} />下载</button><button type="button" disabled={!props.selected} onClick={() => props.onMutate("rename")}>重命名</button><button type="button" disabled={!props.selected || props.selected.kind === "symlink"} onClick={() => props.onMutate("chmod")}>权限</button><button className="danger" type="button" disabled={!props.selected} onClick={() => props.onMutate("delete")}><Trash2 size={13} />删除</button><span className="sftp-drop-hint">可拖放本地文件上传；路径仅由 Rust 处理</span></div><div className="sftp-table" role="grid" aria-label="远程文件"><div className="sftp-row sftp-heading" role="row"><span>名称</span><span>大小</span><span>权限</span></div>{props.entries.map((entry) => <button className={`sftp-row ${props.selected?.path === entry.path ? "selected" : ""}`} role="row" type="button" key={entry.path} onClick={() => props.onSelect(entry)} onDoubleClick={() => entry.kind === "directory" && props.onOpenPath(entry.path)}><span>{entry.kind === "directory" ? "📁" : entry.kind === "symlink" ? "↗" : "▤"} {entry.name}{entry.linkTarget ? ` → ${entry.linkTarget}` : ""}</span><span>{entry.size === null ? "—" : formatBytes(entry.size)}</span><span>{entry.permissions === null ? "—" : (entry.permissions & 0o7777).toString(8)}</span></button>)}{!props.entries.length && <div className="empty-list">目录为空</div>}</div></div>;
}

function TransferView({ transfers, onPause, onCancel, onRetry, onClear }: { transfers: TransferSnapshot[]; onPause(id: string): Promise<void>; onCancel(id: string): Promise<void>; onRetry(id: string): Promise<void>; onClear(): Promise<void> }) {
  return <div className="transfer-view"><div className="transfer-toolbar"><span>最多 2 个并发传输</span><button type="button" onClick={() => void onClear()}>清除已结束</button></div><div className="transfer-list">{transfers.map((value) => <div className="transfer-row" key={value.id}><span className="transfer-direction">{value.direction === "upload" ? <ArrowUpFromLine size={14} /> : <ArrowDownToLine size={14} />}</span><div><strong>{value.name}</strong><small>{value.remotePath} · {transferLabel(value)}</small><progress max={Math.max(1, value.total)} value={value.transferred} /></div><span>{value.total ? `${Math.round(value.transferred / value.total * 100)}%` : "—"}</span>{value.status === "paused" ? <><button className="icon-button" type="button" aria-label={`恢复 ${value.name}`} onClick={() => void onRetry(value.id)}><RotateCcw size={14} /></button><button className="icon-button" type="button" aria-label={`取消 ${value.name}`} onClick={() => void onCancel(value.id)}><Square size={13} /></button></> : isTerminal(value) ? <button className="icon-button" type="button" aria-label={`重试 ${value.name}`} disabled={value.status === "completed"} onClick={() => void onRetry(value.id)}><RotateCcw size={14} /></button> : <><button className="icon-button" type="button" aria-label={`暂停 ${value.name}`} disabled={!['queued', 'running'].includes(value.status)} onClick={() => void onPause(value.id)}><Pause size={13} /></button><button className="icon-button" type="button" aria-label={`取消 ${value.name}`} disabled={value.status === "cancelling"} onClick={() => void onCancel(value.id)}><Square size={13} /></button></>}</div>)}{!transfers.length && <div className="empty-list">暂无传输任务</div>}</div></div>;
}

function TunnelView({ bridge, connections, defaultConnectionId, tunnels, onTunnels, onError, onActivity }: { bridge: WorkspaceBridge; connections: ConnectionProfile[]; defaultConnectionId: string; tunnels: DesktopTunnelSnapshot[]; onTunnels(value: DesktopTunnelSnapshot[]): void; onError(value: string | null): void; onActivity(value: string): void }) {
  const [connectionId, setConnectionId] = useState(defaultConnectionId || connections[0]?.id || "");
  const [kind, setKind] = useState<TunnelKind>("local");
  const [bindHost, setBindHost] = useState("127.0.0.1");
  const [bindPort, setBindPort] = useState(0);
  const [targetHost, setTargetHost] = useState("127.0.0.1");
  const [targetPort, setTargetPort] = useState(8080);
  const [confirmed, setConfirmed] = useState(false);
  const submit = async (event: FormEvent) => {
    event.preventDefault(); onError(null);
    try {
      const value = await bridge.startTunnel(connectionId, { kind, bindHost, bindPort, targetHost: kind === "dynamic" ? "" : targetHost, targetPort: kind === "dynamic" ? 0 : targetPort, allowNonLoopback: confirmed });
      onTunnels([value, ...tunnels]); onActivity(value.state === "running" ? "SSH 隧道已启动" : "隧道等待主机密钥确认");
    } catch (reason) { onError(message(reason)); }
  };
  const approve = async (value: DesktopTunnelSnapshot) => { if (!value.hostKey) return; try { const next = await bridge.approveTunnelHostKey(value.id, value.hostKey.fingerprint, value.hostKey.previousFingerprint); onTunnels(tunnels.map((item) => item.id === value.id ? next : item)); } catch (reason) { onError(message(reason)); } };
  return <div className="tunnel-view"><form className="tunnel-form" onSubmit={(event) => void submit(event)}><select aria-label="隧道连接" value={connectionId} onChange={(event) => setConnectionId(event.target.value)}>{connections.map((value) => <option key={value.id} value={value.id}>{value.name}</option>)}</select><select aria-label="隧道类型" value={kind} onChange={(event) => setKind(event.target.value as TunnelKind)}><option value="local">本地转发</option><option value="remote">远程转发</option><option value="dynamic">动态 SOCKS5</option></select><input aria-label="监听地址" value={bindHost} onChange={(event) => { setBindHost(event.target.value); setConfirmed(false); }} /><input aria-label="监听端口" type="number" min={0} max={65535} value={bindPort} onChange={(event) => setBindPort(Number(event.target.value))} />{kind !== "dynamic" && <><input aria-label="目标主机" value={targetHost} onChange={(event) => setTargetHost(event.target.value)} /><input aria-label="目标端口" type="number" min={1} max={65535} value={targetPort} onChange={(event) => setTargetPort(Number(event.target.value))} /></>}<label className="exposure-confirm"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />允许非 loopback 暴露</label><button className="primary-command" type="submit" disabled={!connectionId}><Play size={13} />启动</button></form><div className="tunnel-list">{tunnels.map((value) => <div className="tunnel-row" key={value.id}><Network size={14} /><div><strong>{value.tunnel ? `${value.tunnel.bindHost}:${value.tunnel.bindPort}` : "等待主机密钥"}</strong><small>{value.tunnel ? `${value.tunnel.kind}${value.tunnel.targetHost ? ` → ${value.tunnel.targetHost}:${value.tunnel.targetPort}` : ""}` : value.hostKey ? `${value.hostKey.host}:${value.hostKey.port}` : ""}</small></div>{value.state === "host_key_required" ? <button type="button" onClick={() => void approve(value)}><KeyRound size={13} />确认密钥</button> : <button className="icon-button" type="button" aria-label="关闭隧道" onClick={() => void bridge.closeTunnel(value.id).then(() => onTunnels(tunnels.filter((item) => item.id !== value.id))).catch((reason) => onError(message(reason)))}><Square size={13} /></button>}</div>)}{!tunnels.length && <div className="empty-list">暂无活动隧道</div>}</div></div>;
}

function HostKeyNotice({ evidence, onApprove, busy }: { evidence: HostKeyEvidence; onApprove(): void; busy: boolean }) {
  return <div className="host-key-notice"><KeyRound size={22} /><div><strong>{evidence.changed ? "远程主机密钥已变化" : "确认远程主机密钥"}</strong><span>{evidence.host}:{evidence.port} · {evidence.algorithm}</span><code>{evidence.fingerprint}</code></div><button className="primary-command" type="button" disabled={busy} onClick={onApprove}>{evidence.changed ? "替换并继续" : "信任并继续"}</button></div>;
}

function mergeTransfers(current: TransferSnapshot[], next: TransferSnapshot[]) {
  const values = new Map(current.map((value) => [value.id, value]));
  next.forEach((value) => {
    const observed = values.get(value.id);
    if (!observed || isNewerTransfer(value, observed)) values.set(value.id, value);
  });
  return [...values.values()];
}

function isNewerTransfer(candidate: TransferSnapshot, observed: TransferSnapshot) {
  if (candidate.generation !== observed.generation) return candidate.generation > observed.generation;
  const statusRank = (status: TransferStatus) => ({ queued: 0, running: 1, pausing: 2, cancelling: 2, paused: 3, completed: 3, failed: 3, cancelled: 3 })[status];
  const candidateRank = statusRank(candidate.status);
  const observedRank = statusRank(observed.status);
  return candidateRank > observedRank || (candidateRank === observedRank && candidate.transferred > observed.transferred);
}
function isTerminal(value: TransferSnapshot) { return ["completed", "failed", "cancelled"].includes(value.status); }
function transferLabel(value: TransferSnapshot) { return value.error ?? ({ queued: "排队中", running: "传输中", pausing: "正在暂停", paused: "已暂停", cancelling: "正在取消", completed: value.resumed ? "已续传完成" : "已完成", failed: "失败", cancelled: "已取消" } as const)[value.status]; }
function formatBytes(value: number) { if (value < 1024) return `${value} B`; if (value < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`; return `${(value / 1024 ** 2).toFixed(1)} MiB`; }
function message(reason: unknown) { return reason instanceof Error ? reason.message : typeof reason === "object" && reason && "message" in reason ? String((reason as { message: unknown }).message) : "操作失败"; }
