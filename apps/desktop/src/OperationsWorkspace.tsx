import { Activity, Archive, ArrowDownToLine, ArrowUp, ArrowUpFromLine, ChevronDown, ChevronRight, FilePenLine, Folder, FolderOpen, FolderPlus, KeyRound, Network, Pause, Play, Radio, RefreshCw, RotateCcw, Settings2, ShieldCheck, Square, Trash2, X } from "lucide-react";
import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { WorkspaceBridge } from "./bridge";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";
import type { ConnectionProfile, DesktopTunnelSnapshot, HostKeyEvidence, RemoteEditDocument, RemoteEntry, SftpWorkspaceSnapshot, TransferOptions, TransferSnapshot, TransferStatus, TunnelKind } from "./types";

interface OperationsWorkspaceProps {
  bridge: WorkspaceBridge;
  connections: ConnectionProfile[];
  activeConnectionId: string | null;
  readyConnectionIds: string[];
  hidden?: boolean;
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
  const [workspaces, setWorkspaces] = useState<Record<string, SftpWorkspaceSnapshot>>({});
  const workspacesRef = useRef(workspaces);
  const autoManagedRef = useRef(new Set<string>());
  const openingRef = useRef(new Set<string>());
  const suppressedAutoRef = useRef(new Set<string>());
  const [path, setPath] = useState("");
  const [entries, setEntries] = useState<RemoteEntry[]>([]);
  const [treeDirectories, setTreeDirectories] = useState<Record<string, RemoteEntry[]>>({});
  const [expandedDirectories, setExpandedDirectories] = useState<Set<string>>(() => new Set());
  const [selected, setSelected] = useState<RemoteEntry | null>(null);
  const [transfers, setTransfers] = useState<TransferSnapshot[]>([]);
  const [tunnels, setTunnels] = useState<DesktopTunnelSnapshot[]>([]);
  const [options, setOptions] = useState(defaultTransferOptions);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editDocument, setEditDocument] = useState<RemoteEditDocument | null>(null);
  const [editContent, setEditContent] = useState("");
  const readyConnectionKey = [...props.readyConnectionIds].sort().join("\u0000");
  const workspace = workspaces[connectionId] ?? null;

  useEffect(() => { workspacesRef.current = workspaces; }, [workspaces]);

  useEffect(() => {
    if (props.activeConnectionId && sshConnections.some((value) => value.id === props.activeConnectionId)) setConnectionId(props.activeConnectionId);
  }, [props.activeConnectionId, sshConnections]);

  useEffect(() => {
    let active = true;
    const persisted = new Set(sshConnections.map((value) => value.id));
    const ready = new Set(props.readyConnectionIds.filter((id) => persisted.has(id)));

    for (const id of [...suppressedAutoRef.current]) {
      if (!ready.has(id)) suppressedAutoRef.current.delete(id);
    }
    for (const id of [...autoManagedRef.current]) {
      if (ready.has(id)) continue;
      autoManagedRef.current.delete(id);
      const stale = workspacesRef.current[id];
      if (stale) {
        void props.bridge.closeSftp(stale.id).catch(() => undefined);
        setWorkspaces((current) => {
          const next = { ...current };
          delete next[id];
          return next;
        });
      }
    }

    ready.forEach((id) => {
      if (workspacesRef.current[id] || openingRef.current.has(id) || suppressedAutoRef.current.has(id)) return;
      openingRef.current.add(id);
      void props.bridge.openSftp(id)
        .then((next) => {
          if (!active) {
            void props.bridge.closeSftp(next.id).catch(() => undefined);
            return;
          }
          autoManagedRef.current.add(id);
          setWorkspaces((current) => ({ ...current, [id]: next }));
          props.onActivity(next.state === "ready" ? "终端 companion SFTP 已连接" : "companion SFTP 等待主机密钥确认");
        })
        .catch((reason) => { if (active) setError(`SFTP 自动连接失败：${message(reason)}`); })
        .finally(() => { openingRef.current.delete(id); });
    });

    return () => { active = false; };
  }, [props.bridge, props.onActivity, readyConnectionKey, sshConnections]);

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
      const nextEntries = await props.bridge.listSftp(target.id, nextPath);
      setEntries(nextEntries);
      setTreeDirectories((current) => ({ ...current, [treeKey(target.id, nextPath)]: nextEntries.filter((entry) => entry.kind === "directory") }));
      setExpandedDirectories((current) => new Set(current).add(treeKey(target.id, nextPath)));
      setPath(nextPath); setSelected(null);
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  }, [props.bridge, workspace]);

  useEffect(() => {
    if (workspace?.state === "ready") void loadDirectory("", workspace);
    else { setEntries([]); setPath(""); setSelected(null); }
  }, [loadDirectory, workspace?.id, workspace?.state]);

  const openSftp = async () => {
    if (!connectionId) return;
    setBusy(true); setError(null);
    try {
      const next = await props.bridge.openSftp(connectionId);
      suppressedAutoRef.current.delete(connectionId);
      setWorkspaces((current) => ({ ...current, [connectionId]: next }));
      props.onActivity(next.state === "ready" ? "SFTP 工作区已连接" : "SFTP 等待主机密钥确认");
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const closeSftp = async () => {
    if (!workspace) return;
    try { await props.bridge.closeSftp(workspace.id); } catch {}
    suppressedAutoRef.current.add(connectionId);
    autoManagedRef.current.delete(connectionId);
    setWorkspaces((current) => {
      const next = { ...current };
      delete next[connectionId];
      return next;
    });
    setEntries([]); setPath(""); setSelected(null);
  };

  const approveSftp = async () => {
    if (!workspace?.hostKey) return;
    const evidence = workspace.hostKey;
    setBusy(true); setError(null);
    try {
      const next = await props.bridge.approveSftpHostKey(workspace.id, evidence.fingerprint, evidence.previousFingerprint);
      setWorkspaces((current) => ({ ...current, [connectionId]: next }));
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const toggleDirectory = async (directory: string) => {
    if (!workspace || workspace.state !== "ready") return;
    const key = treeKey(workspace.id, directory);
    if (expandedDirectories.has(key)) {
      setExpandedDirectories((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
      return;
    }
    setExpandedDirectories((current) => new Set(current).add(key));
    if (treeDirectories[key]) return;
    try {
      const children = await props.bridge.listSftp(workspace.id, directory);
      setTreeDirectories((current) => ({ ...current, [key]: children.filter((entry) => entry.kind === "directory") }));
    } catch (reason) { setError(message(reason)); }
  };

  const mutateEntry = async (kind: "mkdir" | "rename" | "delete" | "chmod", target = selected) => {
    if (!workspace || workspace.state !== "ready") return;
    try {
      if (kind === "mkdir") {
        const name = window.prompt("新目录名称");
        if (!name) return;
        await props.bridge.createSftpDirectory(workspace.id, path, name);
      } else if (kind === "rename" && target) {
        const name = window.prompt("新名称", target.name);
        if (!name || name === target.name) return;
        await props.bridge.renameSftpEntry(workspace.id, target.path, name);
      } else if (kind === "delete" && target) {
        if (!window.confirm(`删除远程项目“${target.name}”？此操作不可撤销。`)) return;
        await props.bridge.deleteSftpEntry(workspace.id, target.path);
      } else if (kind === "chmod" && target) {
        const raw = window.prompt("POSIX 权限（八进制）", ((target.permissions ?? 0o644) & 0o7777).toString(8));
        if (!raw || !/^[0-7]{3,4}$/.test(raw)) return;
        await props.bridge.chmodSftpEntry(workspace.id, target.path, Number.parseInt(raw, 8));
      }
      await loadDirectory(path);
    } catch (reason) { setError(message(reason)); }
  };

  const upload = async (remoteDirectory = path) => {
    if (!workspace || workspace.state !== "ready") return;
    try {
      const queued = await props.bridge.chooseSftpUploads(workspace.id, remoteDirectory, options, updateTransfer);
      setTransfers((current) => mergeTransfers(current, queued));
      if (queued.length) { setTab("transfers"); props.onActivity(`${queued.length} 个上传任务已加入队列`); }
    } catch (reason) { setError(message(reason)); }
  };

  const download = async (target = selected) => {
    if (!workspace || workspace.state !== "ready" || target?.kind !== "file") return;
    try {
      const queued = await props.bridge.chooseSftpDownload(workspace.id, target.path, options, updateTransfer);
      if (queued) { setTransfers((current) => mergeTransfers(current, [queued])); setTab("transfers"); props.onActivity("下载任务已加入队列"); }
    } catch (reason) { setError(message(reason)); }
  };

  const openEditor = async (target = selected) => {
    if (!workspace || workspace.state !== "ready" || target?.kind !== "file") return;
    setBusy(true); setError(null);
    try { const document = await props.bridge.openRemoteText(workspace.id, target.path); setEditDocument(document); setEditContent(document.content); }
    catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const saveEditor = async () => {
    if (!workspace || !editDocument) return;
    setBusy(true); setError(null);
    try {
      const saved = await props.bridge.saveRemoteText(workspace.id, editDocument.path, editDocument.fingerprint, editContent, editDocument.utf8Bom);
      setEditDocument(saved); setEditContent(saved.content); props.onActivity(`远程文件已安全保存：${saved.path}`); await loadDirectory(path);
    } catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const archiveSelected = async (target = selected) => {
    if (!workspace || workspace.state !== "ready" || !target) return;
    const base = target.name.replace(/\.(tar\.gz|tgz)$/i, "");
    const destination = window.prompt("压缩包路径（.tar.gz）", `${path ? `${path}/` : ""}${base}.tar.gz`);
    if (!destination) return;
    setBusy(true); setError(null);
    try { const receipt = await props.bridge.createRemoteArchive(workspace.id, [target.path], destination); props.onActivity(`远程压缩包已创建：${receipt.path}（${formatBytes(receipt.bytes)}）`); await loadDirectory(path); }
    catch (reason) { setError(message(reason)); }
    finally { setBusy(false); }
  };

  const zmodemUpload = async () => {
    if (!workspace || workspace.state !== "ready") return;
    try { const queued = await props.bridge.chooseZmodemUpload(workspace.id, path, updateTransfer); if (queued) { setTransfers((current) => mergeTransfers(current, [queued])); setTab("transfers"); props.onActivity("ZMODEM 上传已加入受控队列"); } }
    catch (reason) { setError(message(reason)); }
  };

  const zmodemDownload = async (target = selected) => {
    if (!workspace || workspace.state !== "ready" || target?.kind !== "file") return;
    try { const queued = await props.bridge.chooseZmodemDownload(workspace.id, target.path, updateTransfer); if (queued) { setTransfers((current) => mergeTransfers(current, [queued])); setTab("transfers"); props.onActivity("ZMODEM 下载已加入受控队列"); } }
    catch (reason) { setError(message(reason)); }
  };

  return (
    <section className="operations-panel" aria-label="文件与网络工具" hidden={props.hidden}>
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
        {tab === "sftp" && <SftpView connections={sshConnections} connectionId={connectionId} onConnectionId={setConnectionId} workspace={workspace} path={path} entries={entries} treeDirectories={treeDirectories} expandedDirectories={expandedDirectories} selected={selected} busy={busy} options={options} onOptions={setOptions} onOpen={() => void openSftp()} onClose={() => void closeSftp()} onApprove={() => void approveSftp()} onOpenPath={(value) => void loadDirectory(value)} onToggleDirectory={(value) => void toggleDirectory(value)} onSelect={setSelected} onRefresh={() => void loadDirectory(path)} onMutate={(value, target) => void mutateEntry(value, target)} onUpload={(directory) => void upload(directory)} onDownload={(target) => void download(target)} onEdit={(target) => void openEditor(target)} onArchive={(target) => void archiveSelected(target)} onZmodemUpload={() => void zmodemUpload()} onZmodemDownload={(target) => void zmodemDownload(target)} />}
        {tab === "transfers" && <TransferView transfers={transfers} onPause={async (id) => { await props.bridge.pauseTransfer(id); setTransfers(await props.bridge.listTransfers()); }} onCancel={async (id) => { await props.bridge.cancelTransfer(id); setTransfers(await props.bridge.listTransfers()); }} onRetry={async (id) => { const value = await props.bridge.retryTransfer(id, updateTransfer); setTransfers((current) => mergeTransfers(current, [value])); }} onClear={async () => { await props.bridge.clearFinishedTransfers(); setTransfers(await props.bridge.listTransfers()); }} />}
        {tab === "tunnels" && <TunnelView bridge={props.bridge} connections={sshConnections} defaultConnectionId={connectionId} tunnels={tunnels} onTunnels={setTunnels} onError={setError} onActivity={props.onActivity} />}
        {tab === "activity" && <div className="activity-list operations-activity">{props.activities.map((item) => <div key={item.id}><time>{item.time}</time><span>{item.message}</span></div>)}{props.activities.length === 0 && <div className="empty-list">暂无活动</div>}</div>}
      </div>
      {editDocument && <div className="remote-editor-backdrop" role="dialog" aria-modal="true" aria-label="远程文本编辑器"><div className="remote-editor"><header><div><strong>{editDocument.path}</strong><small>{formatBytes(editDocument.bytes)} · SHA-256 冲突检测</small></div><button className="icon-button" type="button" aria-label="关闭远程编辑器" onClick={() => setEditDocument(null)}><X size={15} /></button></header><textarea aria-label="远程文件内容" value={editContent} onChange={(event) => setEditContent(event.target.value)} spellCheck={false} /><footer><label><input type="checkbox" checked={editDocument.utf8Bom} onChange={(event) => setEditDocument({ ...editDocument, utf8Bom: event.target.checked })} />保留 UTF-8 BOM</label><span>保存前将重新核对远端指纹</span><button className="primary-command" type="button" disabled={busy || editContent === editDocument.content} onClick={() => void saveEditor()}>安全保存</button></footer></div></div>}
    </section>
  );
}

function Tab({ value, active, onSelect, children }: { value: BottomTab; active: BottomTab; onSelect(value: BottomTab): void; children: React.ReactNode }) {
  return <button type="button" aria-current={active === value} onClick={() => onSelect(value)}>{children}</button>;
}

interface SftpViewProps {
  connections: ConnectionProfile[];
  connectionId: string;
  onConnectionId(value: string): void;
  workspace: SftpWorkspaceSnapshot | null;
  path: string;
  entries: RemoteEntry[];
  treeDirectories: Record<string, RemoteEntry[]>;
  expandedDirectories: Set<string>;
  selected: RemoteEntry | null;
  busy: boolean;
  options: TransferOptions;
  onOptions(value: TransferOptions): void;
  onOpen(): void;
  onClose(): void;
  onApprove(): void;
  onOpenPath(value: string): void;
  onToggleDirectory(value: string): void;
  onSelect(value: RemoteEntry | null): void;
  onRefresh(): void;
  onMutate(value: "mkdir" | "rename" | "delete" | "chmod", target?: RemoteEntry | null): void;
  onUpload(directory?: string): void;
  onDownload(target?: RemoteEntry | null): void;
  onEdit(target?: RemoteEntry | null): void;
  onArchive(target?: RemoteEntry | null): void;
  onZmodemUpload(): void;
  onZmodemDownload(target?: RemoteEntry | null): void;
}

function SftpView(props: SftpViewProps) {
  const [address, setAddress] = useState("");
  const [addressError, setAddressError] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; entry: RemoteEntry | null } | null>(null);
  useEffect(() => {
    setAddress(props.workspace ? remoteAddress(props.workspace.root, props.path) : "");
    setAddressError(null);
  }, [props.path, props.workspace]);

  if (!props.workspace) return <div className="sftp-connect"><select aria-label="SFTP 连接" value={props.connectionId} onChange={(event) => props.onConnectionId(event.target.value)}><option value="">选择 SSH 连接</option>{props.connections.map((value) => <option key={value.id} value={value.id}>{value.name} · {value.host}</option>)}</select><span className="sftp-system-root" title="实际可访问范围由远端 SSH 账户权限决定">根目录 /</span><button className="primary-command" type="button" disabled={!props.connectionId || props.busy} onClick={props.onOpen}>连接 SFTP</button></div>;
  if (props.workspace.state === "host_key_required" && props.workspace.hostKey) return <HostKeyNotice evidence={props.workspace.hostKey} onApprove={props.onApprove} busy={props.busy} />;
  const submitAddress = (event: FormEvent) => {
    event.preventDefault();
    const next = relativeRemoteAddress(props.workspace?.root ?? "", address);
    if (next === null) { setAddressError("地址必须位于当前 SFTP 根目录内"); return; }
    setAddressError(null);
    props.onOpenPath(next);
  };
  const parent = props.path.includes("/") ? props.path.slice(0, props.path.lastIndexOf("/")) : "";
  const menuItems: ContextMenuItem[] = menu?.entry ? [
    ...(menu.entry.kind === "directory" ? [
      { label: "打开目录", icon: <FolderOpen size={15} />, onSelect: () => props.onOpenPath(menu.entry!.path) },
      { label: "上传到此目录", icon: <ArrowUpFromLine size={15} />, onSelect: () => props.onUpload(menu.entry!.path) },
    ] : menu.entry.kind === "file" ? [
      { label: "下载", icon: <ArrowDownToLine size={15} />, onSelect: () => props.onDownload(menu.entry) },
      { label: "编辑文本", icon: <FilePenLine size={15} />, onSelect: () => props.onEdit(menu.entry) },
      { label: "ZMODEM 下载", icon: <Radio size={15} />, onSelect: () => props.onZmodemDownload(menu.entry) },
    ] : []),
    ...((menu.entry.kind === "file" || menu.entry.kind === "directory") ? [{ label: "创建压缩包", icon: <Archive size={15} />, onSelect: () => props.onArchive(menu.entry) }] : []),
    { label: "重命名", icon: <FilePenLine size={15} />, onSelect: () => props.onMutate("rename", menu.entry) },
    ...((menu.entry.kind !== "symlink") ? [{ label: "修改权限", icon: <ShieldCheck size={15} />, onSelect: () => props.onMutate("chmod", menu.entry) }] : []),
    { label: "删除", icon: <Trash2 size={15} />, danger: true, onSelect: () => props.onMutate("delete", menu.entry) },
  ] : [
    { label: "刷新", icon: <RefreshCw size={15} />, onSelect: props.onRefresh },
    { label: "上传文件", icon: <ArrowUpFromLine size={15} />, onSelect: () => props.onUpload(props.path) },
    { label: "新建目录", icon: <FolderPlus size={15} />, onSelect: () => props.onMutate("mkdir") },
    { label: "ZMODEM 上传", icon: <Radio size={15} />, onSelect: props.onZmodemUpload },
  ];
  return <div className="sftp-browser">
    <div className="sftp-toolbar">
      <button className="icon-button" type="button" aria-label="上级目录" title="上级目录" disabled={!props.path} onClick={() => props.onOpenPath(parent)}><ArrowUp size={14} /></button>
      <button className="icon-button" type="button" aria-label="刷新 SFTP" title="刷新" onClick={props.onRefresh}><RefreshCw size={14} /></button>
      <form className="sftp-address" onSubmit={submitAddress}><FolderOpen size={14} aria-hidden="true" /><input aria-label="远程地址" aria-invalid={Boolean(addressError)} value={address} onChange={(event) => setAddress(event.target.value)} title={addressError ?? "输入当前 SFTP 根目录内的路径并回车"} /><button type="submit">跳转</button></form>
      <button className="icon-button" type="button" aria-label="新建远程目录" title="新建目录" onClick={() => props.onMutate("mkdir")}><FolderPlus size={14} /></button>
      <button className="icon-button" type="button" aria-label="多文件上传" title="多文件上传" onClick={() => props.onUpload()}><ArrowUpFromLine size={14} /></button>
      <button className="icon-button" type="button" aria-label="下载" title="下载" disabled={props.selected?.kind !== "file"} onClick={() => props.onDownload()}><ArrowDownToLine size={14} /></button>
      <button className="icon-button" type="button" aria-label="编辑远程文本" title="编辑文本（最大 2 MiB）" disabled={props.selected?.kind !== "file"} onClick={() => props.onEdit()}><FilePenLine size={14} /></button>
      <button className="icon-button" type="button" aria-label="创建远程压缩包" title="创建 .tar.gz" disabled={!props.selected || props.selected.kind === "symlink"} onClick={() => props.onArchive()}><Archive size={14} /></button>
      <button className="icon-button" type="button" aria-label="ZMODEM 上传" title="ZMODEM 上传" onClick={props.onZmodemUpload}><Radio size={14} /></button>
      <button className="icon-button" type="button" aria-label="ZMODEM 下载" title="ZMODEM 下载" disabled={props.selected?.kind !== "file"} onClick={() => props.onZmodemDownload()}><Radio size={14} /></button>
      <button className="icon-button" type="button" aria-label="重命名" title="重命名" disabled={!props.selected} onClick={() => props.onMutate("rename")}><FilePenLine size={14} /></button>
      <button className="icon-button" type="button" aria-label="修改权限" title="修改权限" disabled={!props.selected || props.selected.kind === "symlink"} onClick={() => props.onMutate("chmod")}><ShieldCheck size={14} /></button>
      <button className="icon-button danger" type="button" aria-label="删除" title="删除" disabled={!props.selected} onClick={() => props.onMutate("delete")}><Trash2 size={13} /></button>
      <details className="sftp-options"><summary aria-label="传输选项" title="传输选项"><Settings2 size={14} /></summary><div><label><span>冲突策略</span><select aria-label="冲突策略" value={props.options.conflict} onChange={(event) => props.onOptions({ ...props.options, conflict: event.target.value as TransferOptions["conflict"] })}><option value="deny">拒绝覆盖</option><option value="replace">替换</option><option value="resume">续传</option></select></label><label><span>校验</span><select aria-label="校验方式" value={props.options.verify} onChange={(event) => props.onOptions({ ...props.options, verify: event.target.value as TransferOptions["verify"] })}><option value="size">大小</option><option value="sha256">SHA-256</option></select></label><small>拖放上传路径仅由 Rust 处理</small></div></details>
      <button type="button" onClick={props.onClose}>关闭</button>
    </div>
    <div className="sftp-body">
      <DirectoryTree workspace={props.workspace} currentPath={props.path} directories={props.treeDirectories} expanded={props.expandedDirectories} onOpen={props.onOpenPath} onToggle={props.onToggleDirectory} />
      <div className="sftp-table" role="grid" aria-label="远程文件" onContextMenu={(event) => { event.preventDefault(); props.onSelect(null); setMenu({ x: event.clientX, y: event.clientY, entry: null }); }}><div className="sftp-row sftp-heading" role="row"><span>名称</span><span>大小</span><span>权限</span></div>{props.entries.map((entry) => <button className={`sftp-row ${props.selected?.path === entry.path ? "selected" : ""}`} role="row" type="button" key={entry.path} onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); props.onSelect(entry); setMenu({ x: event.clientX, y: event.clientY, entry }); }} onClick={() => props.onSelect(entry)} onDoubleClick={() => entry.kind === "directory" && props.onOpenPath(entry.path)}><span>{entry.kind === "directory" ? "📁" : entry.kind === "symlink" ? "↗" : "▤"} {entry.name}{entry.linkTarget ? ` → ${entry.linkTarget}` : ""}</span><span>{entry.size === null ? "—" : formatBytes(entry.size)}</span><span>{entry.permissions === null ? "—" : (entry.permissions & 0o7777).toString(8)}</span></button>)}{!props.entries.length && <div className="empty-list">目录为空</div>}</div>
    </div>
    {menu && <ContextMenu x={menu.x} y={menu.y} label={menu.entry ? `${menu.entry.name} 操作` : "当前目录操作"} items={menuItems} onClose={() => setMenu(null)} />}
  </div>;
}

function DirectoryTree({ workspace, currentPath, directories, expanded, onOpen, onToggle }: { workspace: SftpWorkspaceSnapshot; currentPath: string; directories: Record<string, RemoteEntry[]>; expanded: Set<string>; onOpen(path: string): void; onToggle(path: string): void }) {
  const rootKey = treeKey(workspace.id, "");
  return <div className="sftp-tree" role="tree" aria-label="远程目录树"><div className="sftp-tree-root" role="treeitem" aria-expanded={expanded.has(rootKey)}><button className="tree-toggle" type="button" aria-label="展开根目录" onClick={() => onToggle("")}>{expanded.has(rootKey) ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</button><button className={currentPath === "" ? "active" : ""} type="button" onClick={() => onOpen("")}><FolderOpen size={14} /><span>{workspace.root}</span></button></div>{expanded.has(rootKey) && <div role="group">{(directories[rootKey] ?? []).map((entry) => <DirectoryNode key={entry.path} workspaceId={workspace.id} entry={entry} currentPath={currentPath} directories={directories} expanded={expanded} onOpen={onOpen} onToggle={onToggle} />)}</div>}</div>;
}

function DirectoryNode({ workspaceId, entry, currentPath, directories, expanded, onOpen, onToggle }: { workspaceId: string; entry: RemoteEntry; currentPath: string; directories: Record<string, RemoteEntry[]>; expanded: Set<string>; onOpen(path: string): void; onToggle(path: string): void }) {
  const key = treeKey(workspaceId, entry.path);
  const open = expanded.has(key);
  return <div className="sftp-tree-node" role="treeitem" aria-expanded={open}><div><button className="tree-toggle" type="button" aria-label={`${open ? "收起" : "展开"} ${entry.name}`} onClick={() => onToggle(entry.path)}>{open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</button><button className={currentPath === entry.path ? "active" : ""} type="button" onClick={() => onOpen(entry.path)}><Folder size={14} /><span>{entry.name}</span></button></div>{open && <div role="group">{(directories[key] ?? []).map((child) => <DirectoryNode key={child.path} workspaceId={workspaceId} entry={child} currentPath={currentPath} directories={directories} expanded={expanded} onOpen={onOpen} onToggle={onToggle} />)}</div>}</div>;
}

function TransferView({ transfers, onPause, onCancel, onRetry, onClear }: { transfers: TransferSnapshot[]; onPause(id: string): Promise<void>; onCancel(id: string): Promise<void>; onRetry(id: string): Promise<void>; onClear(): Promise<void> }) {
  const [menu, setMenu] = useState<{ x: number; y: number; transfer: TransferSnapshot } | null>(null);
  const menuItems: ContextMenuItem[] = menu ? [
    ...(["queued", "running"].includes(menu.transfer.status) ? [{ label: "暂停传输", icon: <Pause size={15} />, onSelect: () => void onPause(menu.transfer.id) }] : []),
    ...(["paused", "failed", "cancelled"].includes(menu.transfer.status) ? [{ label: "重试传输", icon: <RotateCcw size={15} />, onSelect: () => void onRetry(menu.transfer.id) }] : []),
    ...(!isTerminal(menu.transfer) ? [{ label: "取消传输", icon: <Square size={15} />, danger: true, disabled: menu.transfer.status === "cancelling", onSelect: () => void onCancel(menu.transfer.id) }] : []),
    ...(isTerminal(menu.transfer) ? [{ label: "清除已结束", icon: <Trash2 size={15} />, onSelect: () => void onClear() }] : []),
  ] : [];
  return <div className="transfer-view"><div className="transfer-toolbar"><span>最多 2 个并发传输</span><button type="button" onClick={() => void onClear()}>清除已结束</button></div><div className="transfer-list">{transfers.map((value) => <div className="transfer-row" key={value.id} onContextMenu={(event) => { event.preventDefault(); setMenu({ x: event.clientX, y: event.clientY, transfer: value }); }}><span className="transfer-direction">{value.direction === "upload" ? <ArrowUpFromLine size={14} /> : <ArrowDownToLine size={14} />}</span><div><strong>{value.name}</strong><small>{value.remotePath} · {transferLabel(value)}</small><progress max={Math.max(1, value.total)} value={value.transferred} /></div><span>{value.total ? `${Math.round(value.transferred / value.total * 100)}%` : "—"}</span>{value.status === "paused" ? <><button className="icon-button" type="button" aria-label={`恢复 ${value.name}`} onClick={() => void onRetry(value.id)}><RotateCcw size={14} /></button><button className="icon-button" type="button" aria-label={`取消 ${value.name}`} onClick={() => void onCancel(value.id)}><Square size={13} /></button></> : isTerminal(value) ? <button className="icon-button" type="button" aria-label={`重试 ${value.name}`} disabled={value.status === "completed"} onClick={() => void onRetry(value.id)}><RotateCcw size={14} /></button> : <><button className="icon-button" type="button" aria-label={`暂停 ${value.name}`} disabled={!['queued', 'running'].includes(value.status)} onClick={() => void onPause(value.id)}><Pause size={13} /></button><button className="icon-button" type="button" aria-label={`取消 ${value.name}`} disabled={value.status === "cancelling"} onClick={() => void onCancel(value.id)}><Square size={13} /></button></>}</div>)}{!transfers.length && <div className="empty-list">暂无传输任务</div>}</div>{menu && <ContextMenu x={menu.x} y={menu.y} label={`${menu.transfer.name} 传输操作`} items={menuItems} onClose={() => setMenu(null)} />}</div>;
}

function TunnelView({ bridge, connections, defaultConnectionId, tunnels, onTunnels, onError, onActivity }: { bridge: WorkspaceBridge; connections: ConnectionProfile[]; defaultConnectionId: string; tunnels: DesktopTunnelSnapshot[]; onTunnels(value: DesktopTunnelSnapshot[]): void; onError(value: string | null): void; onActivity(value: string): void }) {
  const [connectionId, setConnectionId] = useState(defaultConnectionId || connections[0]?.id || "");
  const [kind, setKind] = useState<TunnelKind>("local");
  const [bindHost, setBindHost] = useState("127.0.0.1");
  const [bindPort, setBindPort] = useState(0);
  const [targetHost, setTargetHost] = useState("127.0.0.1");
  const [targetPort, setTargetPort] = useState(8080);
  const [confirmed, setConfirmed] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number; tunnel: DesktopTunnelSnapshot } | null>(null);
  const submit = async (event: FormEvent) => {
    event.preventDefault(); onError(null);
    try {
      const value = await bridge.startTunnel(connectionId, { kind, bindHost, bindPort, targetHost: kind === "dynamic" ? "" : targetHost, targetPort: kind === "dynamic" ? 0 : targetPort, allowNonLoopback: confirmed });
      onTunnels([value, ...tunnels]); onActivity(value.state === "running" ? "SSH 隧道已启动" : "隧道等待主机密钥确认");
    } catch (reason) { onError(message(reason)); }
  };
  const approve = async (value: DesktopTunnelSnapshot) => { if (!value.hostKey) return; try { const next = await bridge.approveTunnelHostKey(value.id, value.hostKey.fingerprint, value.hostKey.previousFingerprint); onTunnels(tunnels.map((item) => item.id === value.id ? next : item)); } catch (reason) { onError(message(reason)); } };
  const close = (value: DesktopTunnelSnapshot) => bridge.closeTunnel(value.id).then(() => onTunnels(tunnels.filter((item) => item.id !== value.id))).catch((reason) => onError(message(reason)));
  const menuItems: ContextMenuItem[] = menu ? menu.tunnel.state === "host_key_required" ? [
    { label: "确认主机密钥", icon: <KeyRound size={15} />, onSelect: () => void approve(menu.tunnel) },
  ] : [
    { label: "关闭隧道", icon: <Square size={15} />, danger: true, onSelect: () => void close(menu.tunnel) },
  ] : [];
  return <div className="tunnel-view"><form className="tunnel-form" onSubmit={(event) => void submit(event)}><select aria-label="隧道连接" value={connectionId} onChange={(event) => setConnectionId(event.target.value)}>{connections.map((value) => <option key={value.id} value={value.id}>{value.name}</option>)}</select><select aria-label="隧道类型" value={kind} onChange={(event) => setKind(event.target.value as TunnelKind)}><option value="local">本地转发</option><option value="remote">远程转发</option><option value="dynamic">动态 SOCKS5</option></select><input aria-label="监听地址" value={bindHost} onChange={(event) => { setBindHost(event.target.value); setConfirmed(false); }} /><input aria-label="监听端口" type="number" min={0} max={65535} value={bindPort} onChange={(event) => setBindPort(Number(event.target.value))} />{kind !== "dynamic" && <><input aria-label="目标主机" value={targetHost} onChange={(event) => setTargetHost(event.target.value)} /><input aria-label="目标端口" type="number" min={1} max={65535} value={targetPort} onChange={(event) => setTargetPort(Number(event.target.value))} /></>}<label className="exposure-confirm"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />允许非 loopback 暴露</label><button className="primary-command" type="submit" disabled={!connectionId}><Play size={13} />启动</button></form><div className="tunnel-list">{tunnels.map((value) => <div className="tunnel-row" key={value.id} onContextMenu={(event) => { event.preventDefault(); setMenu({ x: event.clientX, y: event.clientY, tunnel: value }); }}><Network size={14} /><div><strong>{value.tunnel ? `${value.tunnel.bindHost}:${value.tunnel.bindPort}` : "等待主机密钥"}</strong><small>{value.tunnel ? `${value.tunnel.kind}${value.tunnel.targetHost ? ` → ${value.tunnel.targetHost}:${value.tunnel.targetPort}` : ""}` : value.hostKey ? `${value.hostKey.host}:${value.hostKey.port}` : ""}</small></div>{value.state === "host_key_required" ? <button type="button" onClick={() => void approve(value)}><KeyRound size={13} />确认密钥</button> : <button className="icon-button" type="button" aria-label="关闭隧道" onClick={() => void close(value)}><Square size={13} /></button>}</div>)}{!tunnels.length && <div className="empty-list">暂无活动隧道</div>}</div>{menu && <ContextMenu x={menu.x} y={menu.y} label="隧道操作" items={menuItems} onClose={() => setMenu(null)} />}</div>;
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
function treeKey(workspaceId: string, path: string) { return `${workspaceId}\u0000${path}`; }
function remoteAddress(root: string, path: string) { return path ? `${root.replace(/\/$/, "")}/${path}` : root; }
function relativeRemoteAddress(root: string, address: string) {
  const normalizedRoot = root.replace(/\/$/, "") || "/";
  const normalized = address.trim().replace(/\\/g, "/").replace(/\/{2,}/g, "/").replace(/\/$/, "") || normalizedRoot;
  let relative: string | null = null;
  if (normalized === normalizedRoot) relative = "";
  else if (normalizedRoot === "/" && normalized.startsWith("/")) relative = normalized.slice(1);
  else if (normalized.startsWith(`${normalizedRoot}/`)) relative = normalized.slice(normalizedRoot.length + 1);
  else if (!normalized.startsWith("/")) relative = normalized.replace(/^\.\//, "");
  return relative !== null && !relative.split("/").includes("..") ? relative : null;
}
