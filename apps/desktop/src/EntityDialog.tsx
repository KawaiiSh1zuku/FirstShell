import { Save, X } from "lucide-react";
import { FormEvent, useMemo, useState } from "react";
import type {
  ConnectionInput,
  ConnectionProfile,
  Folder,
  FolderInput,
  WorkspaceSnapshot,
} from "./types";

type DialogValue =
  | { kind: "folder"; value?: Folder }
  | { kind: "connection"; value?: ConnectionProfile };

interface EntityDialogProps {
  dialog: DialogValue;
  snapshot: WorkspaceSnapshot;
  busy: boolean;
  error: string | null;
  onClose(): void;
  onSaveFolder(input: FolderInput, existing?: Folder): Promise<void>;
  onSaveConnection(input: ConnectionInput, existing?: ConnectionProfile): Promise<void>;
}

export function EntityDialog(props: EntityDialogProps) {
  const { dialog, snapshot, busy, error, onClose } = props;
  const existingFolder = dialog.kind === "folder" ? dialog.value : undefined;
  const existingConnection = dialog.kind === "connection" ? dialog.value : undefined;
  const [name, setName] = useState(existingFolder?.name ?? existingConnection?.name ?? "");
  const [folderId, setFolderId] = useState(existingConnection?.folderId ?? "");
  const [parentId, setParentId] = useState(existingFolder?.parentId ?? "");
  const [kind, setKind] = useState(existingConnection?.kind ?? "ssh");
  const [host, setHost] = useState(existingConnection?.host ?? "");
  const [port, setPort] = useState(existingConnection?.port ?? 22);
  const [username, setUsername] = useState(existingConnection?.username ?? "");
  const [proxyId, setProxyId] = useState(existingConnection?.proxyId ?? "");
  const [terminalId, setTerminalId] = useState(existingConnection?.terminalProfileId ?? "");
  const [authentication, setAuthentication] = useState<ConnectionInput["authentication"]>(existingConnection?.authentication ?? "agent");
  const [privateKeyPath, setPrivateKeyPath] = useState(existingConnection?.privateKeyPath ?? "");
  const [allowLegacyAlgorithms, setAllowLegacyAlgorithms] = useState(existingConnection?.allowLegacyAlgorithms ?? false);
  const [opensshFallback, setOpensshFallback] = useState(existingConnection?.opensshFallback ?? false);
  const [tags, setTags] = useState(existingConnection?.tags.join(", ") ?? "");
  const [notes, setNotes] = useState(existingFolder?.notes ?? existingConnection?.notes ?? "");

  const availableFolders = useMemo(
    () => snapshot.folders.filter((folder) => folder.id !== existingFolder?.id),
    [snapshot.folders, existingFolder?.id],
  );

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    try {
      if (dialog.kind === "folder") {
        await props.onSaveFolder(
          { parentId: parentId || null, sortOrder: existingFolder?.sortOrder ?? snapshot.folders.length, name, notes },
          existingFolder,
        );
        return;
      }
      const local = kind === "local";
      await props.onSaveConnection(
        {
          folderId: folderId || null,
          sortOrder: existingConnection?.sortOrder ?? snapshot.connections.length,
          kind,
          name,
          host: local ? "" : host,
          port: local ? 0 : Number(port),
          username: local ? "" : username,
          proxyId: proxyId || null,
          terminalProfileId: terminalId || null,
          authentication,
          privateKeyPath: kind === "ssh" && authentication === "private_key" ? privateKeyPath : "",
          allowLegacyAlgorithms: kind === "ssh" && allowLegacyAlgorithms,
          opensshFallback: kind === "ssh" && authentication === "agent" && opensshFallback,
          tags: tags.split(",").map((value) => value.trim()).filter(Boolean),
          notes,
        },
        existingConnection,
      );
    } catch {
      // The parent keeps the dialog open and renders the redacted command error.
    }
  };

  const title = dialog.kind === "folder"
    ? existingFolder ? "编辑文件夹" : "新建文件夹"
    : existingConnection ? "编辑连接" : "新建连接";

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section className="modal" role="dialog" aria-modal="true" aria-labelledby="entity-dialog-title">
        <header className="modal-header">
          <h2 id="entity-dialog-title">{title}</h2>
          <button className="icon-button" type="button" aria-label="关闭" title="关闭" onClick={onClose}><X size={17} /></button>
        </header>
        <form className="entity-form" onSubmit={submit}>
          <label><span>名称</span><input autoFocus required maxLength={128} value={name} onChange={(event) => setName(event.target.value)} /></label>

          {dialog.kind === "folder" ? (
            <label><span>上级文件夹</span><select value={parentId} onChange={(event) => setParentId(event.target.value)}><option value="">根目录</option>{availableFolders.map((folder) => <option key={folder.id} value={folder.id}>{folder.name}</option>)}</select></label>
          ) : (
            <>
              <div className="form-row">
                <label><span>类型</span><select value={kind} onChange={(event) => { const value = event.target.value as ConnectionInput["kind"]; setKind(value); if (value === "ssh" && port === 3389) setPort(22); if (value === "rdp" && port === 22) setPort(3389); }}><option value="ssh">SSH</option><option value="rdp">RDP</option><option value="local">本地终端</option></select></label>
                <label><span>文件夹</span><select value={folderId} onChange={(event) => setFolderId(event.target.value)}><option value="">根目录</option>{snapshot.folders.map((folder) => <option key={folder.id} value={folder.id}>{folder.name}</option>)}</select></label>
              </div>
              {kind !== "local" && (
                <>
                  <div className="form-row endpoint-row">
                    <label><span>主机</span><input required maxLength={253} value={host} onChange={(event) => setHost(event.target.value)} /></label>
                    <label><span>端口</span><input type="number" min={1} max={65535} required value={port} onChange={(event) => setPort(Number(event.target.value))} /></label>
                  </div>
                  <label><span>用户名</span><input maxLength={128} value={username} onChange={(event) => setUsername(event.target.value)} /></label>
                </>
              )}
              <div className="form-row">
                <label><span>代理</span><select value={proxyId} onChange={(event) => setProxyId(event.target.value)}><option value="">Direct</option>{snapshot.proxies.map((proxy) => <option key={proxy.id} value={proxy.id}>{proxy.name}</option>)}</select></label>
                <label><span>终端配置</span><select value={terminalId} onChange={(event) => setTerminalId(event.target.value)}><option value="">默认</option>{snapshot.terminalProfiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name}</option>)}</select></label>
              </div>
              {kind === "ssh" && <>
                <div className="form-row">
                  <label><span>认证</span><select value={authentication} onChange={(event) => setAuthentication(event.target.value as ConnectionInput["authentication"])}><option value="agent">SSH Agent</option><option value="password">密码</option><option value="private_key">私钥</option><option value="keyboard_interactive">Keyboard Interactive</option></select></label>
                  {authentication === "private_key" ? <label><span>私钥路径</span><input required maxLength={4096} value={privateKeyPath} onChange={(event) => setPrivateKeyPath(event.target.value)} /></label> : <span />}
                </div>
                <div className="binary-options">
                  <label><input type="checkbox" checked={allowLegacyAlgorithms} onChange={(event) => setAllowLegacyAlgorithms(event.target.checked)} /><span>允许此主机使用兼容算法</span></label>
                  <label><input type="checkbox" checked={authentication === "agent" && opensshFallback} disabled={authentication !== "agent"} onChange={(event) => setOpensshFallback(event.target.checked)} /><span>允许 OpenSSH 兼容回退（仅 Agent）</span></label>
                </div>
              </>}
              <label><span>标签</span><input maxLength={512} value={tags} onChange={(event) => setTags(event.target.value)} /></label>
            </>
          )}

          <label><span>备注</span><textarea maxLength={4096} rows={3} value={notes} onChange={(event) => setNotes(event.target.value)} /></label>
          {error && <div className="inline-error" role="alert">{error}</div>}
          <footer className="modal-actions"><button type="button" onClick={onClose}>取消</button><button className="primary-command" type="submit" disabled={busy}><Save size={16} /><span>{busy ? "保存中" : "保存"}</span></button></footer>
        </form>
      </section>
    </div>
  );
}
