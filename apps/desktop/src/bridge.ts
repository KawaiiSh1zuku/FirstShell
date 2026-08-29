import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  ArchiveReceipt,
  CommandHistoryEntry,
  ConnectionInput,
  ConnectionProfile,
  DesktopTunnelSnapshot,
  ExportReceipt,
  Folder,
  FolderInput,
  ImportSummary,
  MonitorEvent,
  MonitorSnapshot,
  ProbeKind,
  ProbeResult,
  ProxyInput,
  ProxyProfile,
  RemoteEntry,
  RemoteEditDocument,
  RestoreReceipt,
  DesktopSessionEvent,
  HostKeyEvidence,
  QuickConnectInput,
  SearchResult,
  TerminalProfile,
  TerminalProfileInput,
  SessionLogReceipt,
  SessionSnapshot,
  Snippet,
  SnippetInput,
  TerminalSize,
  TransferOptions,
  TransferSnapshot,
  TunnelSpec,
  SftpWorkspaceSnapshot,
  StagedUploadDrop,
  VaultStatus,
  WorkspaceLayout,
  WorkspaceSnapshot,
} from "./types";

export interface WorkspaceBridge {
  status(): Promise<VaultStatus>;
  initialize(password: string): Promise<WorkspaceSnapshot>;
  unlock(): Promise<WorkspaceSnapshot>;
  recover(password: string): Promise<WorkspaceSnapshot>;
  lock(): Promise<void>;
  snapshot(): Promise<WorkspaceSnapshot>;
  createFolder(input: FolderInput): Promise<Folder>;
  updateFolder(id: string, revision: number, input: FolderInput): Promise<Folder>;
  deleteFolder(id: string, revision: number): Promise<void>;
  restoreFolder(id: string, revision: number): Promise<Folder>;
  createConnection(input: ConnectionInput): Promise<ConnectionProfile>;
  updateConnection(id: string, revision: number, input: ConnectionInput): Promise<ConnectionProfile>;
  deleteConnection(id: string, revision: number): Promise<void>;
  restoreConnection(id: string, revision: number): Promise<ConnectionProfile>;
  createProxy(input: ProxyInput): Promise<ProxyProfile>;
  updateProxy(id: string, revision: number, input: ProxyInput): Promise<ProxyProfile>;
  deleteProxy(id: string, revision: number): Promise<void>;
  createTerminal(input: TerminalProfileInput): Promise<TerminalProfile>;
  updateTerminal(id: string, revision: number, input: TerminalProfileInput): Promise<TerminalProfile>;
  deleteTerminal(id: string, revision: number): Promise<void>;
  createSnippet(input: SnippetInput): Promise<Snippet>;
  updateSnippet(id: string, revision: number, input: SnippetInput): Promise<Snippet>;
  deleteSnippet(id: string, revision: number): Promise<void>;
  renderSnippet(id: string, values: Record<string, string>): Promise<string>;
  recordCommand(connectionId: string, command: string): Promise<CommandHistoryEntry>;
  commandHistory(connectionId: string | null, limit: number): Promise<CommandHistoryEntry[]>;
  clearCommandHistory(): Promise<void>;
  search(query: string): Promise<SearchResult[]>;
  loadLayout(): Promise<WorkspaceLayout>;
  saveLayout(layout: WorkspaceLayout): Promise<void>;
  exportWorkspace(password: string): Promise<ExportReceipt>;
  validateImport(fileName: string, password: string): Promise<ImportSummary>;
  restoreImport(fileName: string, password: string): Promise<RestoreReceipt>;
  onVaultLocked(callback: () => void): Promise<() => void>;
  configureCredential(connectionId: string, secret: string): Promise<void>;
  credentialStatus(connectionId: string): Promise<boolean>;
  configureProxyCredential(proxyId: string, secret: string): Promise<void>;
  proxyCredentialStatus(proxyId: string): Promise<boolean>;
  openTerminal(connectionId: string, size: TerminalSize, onEvent: (value: DesktopSessionEvent) => void): Promise<SessionSnapshot>;
  openQuickTerminal(input: QuickConnectInput, size: TerminalSize, onEvent: (value: DesktopSessionEvent) => void): Promise<SessionSnapshot>;
  listTerminals(): Promise<SessionSnapshot[]>;
  sendTerminalInput(sessionId: string, data: number[]): Promise<void>;
  broadcastTerminal(sessionIds: string[], command: string, confirmed: boolean): Promise<void>;
  stopBroadcast(sessionIds: string[]): Promise<void>;
  resizeTerminal(sessionId: string, size: TerminalSize): Promise<void>;
  reconnectTerminal(sessionId: string): Promise<void>;
  closeTerminal(sessionId: string): Promise<void>;
  pendingHostKey(sessionId: string): Promise<HostKeyEvidence | null>;
  approveHostKey(sessionId: string, fingerprint: string, previousFingerprint: string | null): Promise<void>;
  startSessionLog(sessionId: string): Promise<SessionLogReceipt>;
  stopSessionLog(sessionId: string): Promise<void>;
  openSftp(connectionId: string): Promise<SftpWorkspaceSnapshot>;
  approveSftpHostKey(workspaceId: string, fingerprint: string, previousFingerprint: string | null): Promise<SftpWorkspaceSnapshot>;
  closeSftp(workspaceId: string): Promise<void>;
  listSftp(workspaceId: string, path: string): Promise<RemoteEntry[]>;
  createSftpDirectory(workspaceId: string, parent: string, name: string): Promise<void>;
  renameSftpEntry(workspaceId: string, path: string, name: string): Promise<string>;
  deleteSftpEntry(workspaceId: string, path: string): Promise<void>;
  chmodSftpEntry(workspaceId: string, path: string, mode: number): Promise<void>;
  openRemoteText(workspaceId: string, path: string): Promise<RemoteEditDocument>;
  saveRemoteText(workspaceId: string, path: string, expectedFingerprint: string, content: string, utf8Bom: boolean): Promise<RemoteEditDocument>;
  createRemoteArchive(workspaceId: string, sources: string[], destination: string): Promise<ArchiveReceipt>;
  chooseSftpUploads(workspaceId: string, remoteDirectory: string, options: TransferOptions, onEvent: (value: TransferSnapshot) => void): Promise<TransferSnapshot[]>;
  onSftpUploadDrop(callback: (value: StagedUploadDrop) => void): Promise<() => void>;
  queueSftpDrop(workspaceId: string, batchId: string, remoteDirectory: string, options: TransferOptions, onEvent: (value: TransferSnapshot) => void): Promise<TransferSnapshot[]>;
  chooseSftpDownload(workspaceId: string, remotePath: string, options: TransferOptions, onEvent: (value: TransferSnapshot) => void): Promise<TransferSnapshot | null>;
  chooseZmodemUpload(workspaceId: string, remoteDirectory: string, onEvent: (value: TransferSnapshot) => void): Promise<TransferSnapshot | null>;
  chooseZmodemDownload(workspaceId: string, remotePath: string, onEvent: (value: TransferSnapshot) => void): Promise<TransferSnapshot | null>;
  listTransfers(): Promise<TransferSnapshot[]>;
  pauseTransfer(transferId: string): Promise<void>;
  cancelTransfer(transferId: string): Promise<void>;
  retryTransfer(transferId: string, onEvent: (value: TransferSnapshot) => void): Promise<TransferSnapshot>;
  clearFinishedTransfers(): Promise<void>;
  startTunnel(connectionId: string, spec: TunnelSpec): Promise<DesktopTunnelSnapshot>;
  approveTunnelHostKey(tunnelId: string, fingerprint: string, previousFingerprint: string | null): Promise<DesktopTunnelSnapshot>;
  listTunnels(): Promise<DesktopTunnelSnapshot[]>;
  closeTunnel(tunnelId: string): Promise<void>;
  startHostMonitor(connectionId: string, intervalSeconds: number, onEvent: (value: MonitorEvent) => void): Promise<MonitorSnapshot>;
  approveMonitorHostKey(monitorId: string, fingerprint: string, previousFingerprint: string | null): Promise<MonitorSnapshot>;
  monitorHistory(monitorId: string, maxPoints: number): Promise<ProbeResult[]>;
  runHostProbe(connectionId: string, probe: ProbeKind): Promise<ProbeResult>;
  stopHostMonitor(monitorId: string): Promise<void>;
}

const tauriBridge: WorkspaceBridge = {
  status: () => invoke("vault_status"),
  initialize: (recoveryPassword) => invoke("initialize_vault", { recoveryPassword }),
  unlock: () => invoke("unlock_vault"),
  recover: (recoveryPassword) => invoke("recover_vault", { recoveryPassword }),
  lock: () => invoke("lock_vault"),
  snapshot: () => invoke("workspace_snapshot"),
  createFolder: (input) => invoke("create_folder", { input }),
  updateFolder: (id, expectedRevision, input) =>
    invoke("update_folder", { id, expectedRevision, input }),
  deleteFolder: (id, expectedRevision) => invoke("delete_folder", { id, expectedRevision }),
  restoreFolder: (id, expectedRevision) => invoke("restore_folder", { id, expectedRevision }),
  createConnection: (input) => invoke("create_connection", { input }),
  updateConnection: (id, expectedRevision, input) =>
    invoke("update_connection", { id, expectedRevision, input }),
  deleteConnection: (id, expectedRevision) =>
    invoke("delete_connection", { id, expectedRevision }),
  restoreConnection: (id, expectedRevision) =>
    invoke("restore_connection", { id, expectedRevision }),
  createProxy: (input) => invoke("create_proxy", { input }),
  updateProxy: (id, expectedRevision, input) =>
    invoke("update_proxy", { id, expectedRevision, input }),
  deleteProxy: (id, expectedRevision) => invoke("delete_proxy", { id, expectedRevision }),
  createTerminal: (input) => invoke("create_terminal_profile", { input }),
  updateTerminal: (id, expectedRevision, input) =>
    invoke("update_terminal_profile", { id, expectedRevision, input }),
  deleteTerminal: (id, expectedRevision) =>
    invoke("delete_terminal_profile", { id, expectedRevision }),
  createSnippet: (input) => invoke("create_snippet", { input }),
  updateSnippet: (id, expectedRevision, input) => invoke("update_snippet", { id, expectedRevision, input }),
  deleteSnippet: (id, expectedRevision) => invoke("delete_snippet", { id, expectedRevision }),
  renderSnippet: (id, values) => invoke("render_snippet", { id, values }),
  recordCommand: (connectionId, command) => invoke("record_command_history", { connectionId, command }),
  commandHistory: (connectionId, limit) => invoke("list_command_history", { connectionId, limit }),
  clearCommandHistory: () => invoke("clear_command_history"),
  search: (query) => invoke("search_workspace", { query, limit: 50 }),
  loadLayout: () => invoke("load_workspace_layout"),
  saveLayout: (layout) => invoke("save_workspace_layout", { layout }),
  exportWorkspace: (password) => invoke("export_workspace", { password }),
  validateImport: (fileName, password) =>
    invoke("validate_workspace_import", { fileName, password }),
  restoreImport: (fileName, password) =>
    invoke("restore_workspace_import", { fileName, password }),
  onVaultLocked: async (callback) => listen("vault-locked", callback),
  configureCredential: (connectionId, secret) => invoke("configure_connection_credential", { connectionId, secret }),
  credentialStatus: (connectionId) => invoke("connection_credential_status", { connectionId }),
  configureProxyCredential: (proxyId, secret) => invoke("configure_proxy_credential", { proxyId, secret }),
  proxyCredentialStatus: (proxyId) => invoke("proxy_credential_status", { proxyId }),
  openTerminal: (connectionId, size, onEvent) => {
    const channel = new Channel<DesktopSessionEvent>();
    channel.onmessage = onEvent;
    return invoke("open_terminal_session", { connectionId, size, onEvent: channel });
  },
  openQuickTerminal: (input, size, onEvent) => {
    const channel = new Channel<DesktopSessionEvent>();
    channel.onmessage = onEvent;
    return invoke("open_quick_terminal", { input, size, onEvent: channel });
  },
  listTerminals: () => invoke("list_terminal_sessions"),
  sendTerminalInput: (sessionId, data) => invoke("send_terminal_input", { sessionId, data }),
  broadcastTerminal: (sessionIds, command, confirmed) => invoke("broadcast_terminal_command", { sessionIds, command, confirmed }),
  stopBroadcast: (sessionIds) => invoke("stop_terminal_broadcast", { sessionIds }),
  resizeTerminal: (sessionId, size) => invoke("resize_terminal", { sessionId, size }),
  reconnectTerminal: (sessionId) => invoke("reconnect_terminal", { sessionId }),
  closeTerminal: (sessionId) => invoke("close_terminal", { sessionId }),
  pendingHostKey: (sessionId) => invoke("pending_host_key", { sessionId }),
  approveHostKey: (sessionId, fingerprint, previousFingerprint) =>
    invoke("approve_host_key", { sessionId, fingerprint, previousFingerprint }),
  startSessionLog: (sessionId) => invoke("start_session_log", { sessionId }),
  stopSessionLog: (sessionId) => invoke("stop_session_log", { sessionId }),
  openSftp: (connectionId) => invoke("open_sftp_workspace", { connectionId }),
  approveSftpHostKey: (workspaceId, fingerprint, previousFingerprint) => invoke("approve_sftp_host_key", { workspaceId, fingerprint, previousFingerprint }),
  closeSftp: (workspaceId) => invoke("close_sftp_workspace", { workspaceId }),
  listSftp: (workspaceId, path) => invoke("list_sftp_directory", { workspaceId, path }),
  createSftpDirectory: (workspaceId, parent, name) => invoke("create_sftp_directory", { workspaceId, parent, name }),
  renameSftpEntry: (workspaceId, path, name) => invoke("rename_sftp_entry", { workspaceId, path, name }),
  deleteSftpEntry: (workspaceId, path) => invoke("delete_sftp_entry", { workspaceId, path }),
  chmodSftpEntry: (workspaceId, path, mode) => invoke("chmod_sftp_entry", { workspaceId, path, mode }),
  openRemoteText: (workspaceId, path) => invoke("open_remote_text", { workspaceId, path }),
  saveRemoteText: (workspaceId, path, expectedFingerprint, content, utf8Bom) => invoke("save_remote_text", { workspaceId, path, expectedFingerprint, content, utf8Bom }),
  createRemoteArchive: (workspaceId, sources, destination) => invoke("create_remote_archive", { workspaceId, sources, destination }),
  chooseSftpUploads: (workspaceId, remoteDirectory, options, onEvent) => {
    const channel = new Channel<TransferSnapshot>();
    channel.onmessage = onEvent;
    return invoke("choose_sftp_uploads", { workspaceId, remoteDirectory, options, onEvent: channel });
  },
  onSftpUploadDrop: async (callback) => listen<StagedUploadDrop>("sftp-upload-drop", (event) => callback(event.payload)),
  queueSftpDrop: (workspaceId, batchId, remoteDirectory, options, onEvent) => {
    const channel = new Channel<TransferSnapshot>();
    channel.onmessage = onEvent;
    return invoke("queue_sftp_drop", { workspaceId, batchId, remoteDirectory, options, onEvent: channel });
  },
  chooseSftpDownload: (workspaceId, remotePath, options, onEvent) => {
    const channel = new Channel<TransferSnapshot>();
    channel.onmessage = onEvent;
    return invoke("choose_sftp_download", { workspaceId, remotePath, options, onEvent: channel });
  },
  chooseZmodemUpload: (workspaceId, remoteDirectory, onEvent) => {
    const channel = new Channel<TransferSnapshot>();
    channel.onmessage = onEvent;
    return invoke("choose_zmodem_upload", { workspaceId, remoteDirectory, onEvent: channel });
  },
  chooseZmodemDownload: (workspaceId, remotePath, onEvent) => {
    const channel = new Channel<TransferSnapshot>();
    channel.onmessage = onEvent;
    return invoke("choose_zmodem_download", { workspaceId, remotePath, onEvent: channel });
  },
  listTransfers: () => invoke("list_transfers"),
  pauseTransfer: (transferId) => invoke("pause_transfer", { transferId }),
  cancelTransfer: (transferId) => invoke("cancel_transfer", { transferId }),
  retryTransfer: (transferId, onEvent) => {
    const channel = new Channel<TransferSnapshot>();
    channel.onmessage = onEvent;
    return invoke("retry_transfer", { transferId, onEvent: channel });
  },
  clearFinishedTransfers: () => invoke("clear_finished_transfers"),
  startTunnel: (connectionId, spec) => invoke("start_tunnel", { connectionId, spec }),
  approveTunnelHostKey: (tunnelId, fingerprint, previousFingerprint) => invoke("approve_tunnel_host_key", { tunnelId, fingerprint, previousFingerprint }),
  listTunnels: () => invoke("list_tunnels"),
  closeTunnel: (tunnelId) => invoke("close_tunnel", { tunnelId }),
  startHostMonitor: (connectionId, intervalSeconds, onEvent) => {
    const channel = new Channel<MonitorEvent>();
    channel.onmessage = onEvent;
    return invoke("start_host_monitor", { connectionId, intervalSeconds, onEvent: channel });
  },
  approveMonitorHostKey: (monitorId, fingerprint, previousFingerprint) => invoke("approve_monitor_host_key", { monitorId, fingerprint, previousFingerprint }),
  monitorHistory: (monitorId, maxPoints) => invoke("monitor_history", { monitorId, maxPoints }),
  runHostProbe: (connectionId, probe) => invoke("run_host_probe", { connectionId, probe }),
  stopHostMonitor: (monitorId) => invoke("stop_host_monitor", { monitorId }),
};

const previewSnapshot: WorkspaceSnapshot = {
  folders: [
    { id: "folder-prod", parentId: null, revision: 1, sortOrder: 0, name: "生产环境", notes: "" },
    { id: "folder-lab", parentId: null, revision: 1, sortOrder: 1, name: "实验室", notes: "" },
  ],
  connections: [
    {
      id: "connection-edge",
      folderId: "folder-prod",
      revision: 1,
      sortOrder: 0,
      kind: "ssh",
      name: "Edge Gateway",
      host: "10.24.8.11",
      port: 22,
      username: "operator",
      proxyId: null,
      jumpHostIds: [],
      terminalProfileId: "terminal-default",
      authentication: "agent",
      privateKeyPath: "",
      allowLegacyAlgorithms: false,
      opensshFallback: false,
      tags: ["linux", "gateway"],
      notes: "",
    },
    {
      id: "connection-db",
      folderId: "folder-prod",
      revision: 1,
      sortOrder: 1,
      kind: "ssh",
      name: "Database Primary",
      host: "10.24.12.30",
      port: 22,
      username: "dba",
      proxyId: null,
      jumpHostIds: [],
      terminalProfileId: "terminal-default",
      authentication: "agent",
      privateKeyPath: "",
      allowLegacyAlgorithms: false,
      opensshFallback: false,
      tags: ["postgres"],
      notes: "",
    },
    {
      id: "connection-win",
      folderId: "folder-lab",
      revision: 1,
      sortOrder: 0,
      kind: "rdp",
      name: "Windows Lab",
      host: "192.168.56.20",
      port: 3389,
      username: "Administrator",
      proxyId: null,
      jumpHostIds: [],
      terminalProfileId: null,
      authentication: "agent",
      privateKeyPath: "",
      allowLegacyAlgorithms: false,
      opensshFallback: false,
      tags: ["windows"],
      notes: "",
    },
  ],
  proxies: [],
  terminalProfiles: [
    {
      id: "terminal-default",
      revision: 1,
      name: "Default",
      fontFamily: "Cascadia Mono",
      fontSize: 14,
      theme: "firstshell-dark",
      palette: null,
      scrollback: 10000,
      encoding: "utf8",
      keymap: "xterm",
    },
  ],
  snippets: [
    {
      id: "snippet-preview",
      revision: 1,
      name: "查看服务日志",
      tags: ["systemd", "日志"],
      template: "journalctl -u {{service}} -n {{lines}}",
      parameters: [
        { name: "service", label: "服务", kind: "text", required: true, defaultValue: "nginx", choices: [] },
        { name: "lines", label: "行数", kind: "integer", required: true, defaultValue: "100", choices: [] },
      ],
    },
  ],
};

export function createPreviewBridge(): WorkspaceBridge {
  let snapshot = structuredClone(previewSnapshot);
  const deletedFolders = new Map<string, Folder>();
  const deletedConnections = new Map<string, ConnectionProfile>();
  let layout: WorkspaceLayout = {
    leftWidth: 256,
    leftOpen: true,
    rightWidth: 288,
    bottomHeight: 260,
    bottomOpen: true,
  };
  let sequence = 1;
  const sessions = new Map<string, SessionSnapshot>();
  let transfers: TransferSnapshot[] = [];
  let tunnels: DesktopTunnelSnapshot[] = [];
  let history: CommandHistoryEntry[] = [];
  const monitors = new Map<string, ProbeResult[]>();
  const nextId = (prefix: string) => `${prefix}-preview-${sequence++}`;
  const updateSnapshot = <T>(operation: () => T) => Promise.resolve(operation());

  return {
    status: async () => ({
      initialized: true,
      unlocked: true,
      initializedThisRun: false,
      keyringAvailable: true,
      idleTimeoutSeconds: 900,
    }),
    initialize: async () => structuredClone(snapshot),
    unlock: async () => structuredClone(snapshot),
    recover: async () => structuredClone(snapshot),
    lock: async () => undefined,
    snapshot: async () => structuredClone(snapshot),
    createFolder: (input) =>
      updateSnapshot(() => {
        const value = { id: nextId("folder"), revision: 1, ...input };
        snapshot.folders.push(value);
        return value;
      }),
    updateFolder: (id, revision, input) =>
      updateSnapshot(() => {
        const index = snapshot.folders.findIndex((value) => value.id === id);
        const value = { id, revision: revision + 1, ...input };
        snapshot.folders[index] = value;
        return value;
      }),
    deleteFolder: (id) =>
      updateSnapshot(() => {
        const value = snapshot.folders.find((item) => item.id === id);
        if (value) deletedFolders.set(id, { ...value, revision: value.revision + 1 });
        snapshot.folders = snapshot.folders.filter((value) => value.id !== id);
      }),
    restoreFolder: (id) =>
      updateSnapshot(() => {
        const deleted = deletedFolders.get(id);
        if (!deleted) throw new Error("folder tombstone is unavailable");
        const value = { ...deleted, revision: deleted.revision + 1 };
        deletedFolders.delete(id);
        snapshot.folders.push(value);
        return value;
      }),
    createConnection: (input) =>
      updateSnapshot(() => {
        const value = { id: nextId("connection"), revision: 1, ...input };
        snapshot.connections.push(value);
        return value;
      }),
    updateConnection: (id, revision, input) =>
      updateSnapshot(() => {
        const index = snapshot.connections.findIndex((value) => value.id === id);
        const value = { id, revision: revision + 1, ...input };
        snapshot.connections[index] = value;
        return value;
      }),
    deleteConnection: (id) =>
      updateSnapshot(() => {
        const value = snapshot.connections.find((item) => item.id === id);
        if (value) deletedConnections.set(id, { ...value, revision: value.revision + 1 });
        snapshot.connections = snapshot.connections.filter((value) => value.id !== id);
      }),
    restoreConnection: (id) =>
      updateSnapshot(() => {
        const deleted = deletedConnections.get(id);
        if (!deleted) throw new Error("connection tombstone is unavailable");
        const value = { ...deleted, revision: deleted.revision + 1 };
        deletedConnections.delete(id);
        snapshot.connections.push(value);
        return value;
      }),
    createProxy: (input) =>
      updateSnapshot(() => {
        const value = { id: nextId("proxy"), revision: 1, ...input };
        snapshot.proxies.push(value);
        return value;
      }),
    updateProxy: (id, revision, input) =>
      updateSnapshot(() => {
        const index = snapshot.proxies.findIndex((value) => value.id === id);
        const value = { id, revision: revision + 1, ...input };
        snapshot.proxies[index] = value;
        return value;
      }),
    deleteProxy: (id) =>
      updateSnapshot(() => {
        snapshot.proxies = snapshot.proxies.filter((value) => value.id !== id);
      }),
    createTerminal: (input) =>
      updateSnapshot(() => {
        const value = { id: nextId("terminal"), revision: 1, ...input };
        snapshot.terminalProfiles.push(value);
        return value;
      }),
    updateTerminal: (id, revision, input) =>
      updateSnapshot(() => {
        const index = snapshot.terminalProfiles.findIndex((value) => value.id === id);
        const value = { id, revision: revision + 1, ...input };
        snapshot.terminalProfiles[index] = value;
        return value;
      }),
    deleteTerminal: (id) =>
      updateSnapshot(() => {
        snapshot.terminalProfiles = snapshot.terminalProfiles.filter((value) => value.id !== id);
      }),
    createSnippet: (input) => updateSnapshot(() => {
      const value = { id: nextId("snippet"), revision: 1, ...input };
      snapshot.snippets.push(value);
      return value;
    }),
    updateSnippet: (id, revision, input) => updateSnapshot(() => {
      const index = snapshot.snippets.findIndex((value) => value.id === id);
      const value = { id, revision: revision + 1, ...input };
      snapshot.snippets[index] = value;
      return value;
    }),
    deleteSnippet: (id) => updateSnapshot(() => {
      snapshot.snippets = snapshot.snippets.filter((value) => value.id !== id);
    }),
    renderSnippet: async (id, values) => {
      const snippet = snapshot.snippets.find((value) => value.id === id);
      if (!snippet) throw new Error("snippet not found");
      return snippet.parameters.reduce((text, parameter) => text.replaceAll(`{{${parameter.name}}}`, values[parameter.name] ?? parameter.defaultValue), snippet.template);
    },
    recordCommand: async (connectionId, command) => {
      const value = { connectionId, command, path: command.startsWith("cd ") ? command.slice(3).trim() : null, recordedAtMs: Date.now() };
      history = [...history.filter((item) => item.connectionId !== connectionId || item.command !== command), value].slice(-500);
      return value;
    },
    commandHistory: async (connectionId, limit) => history.filter((value) => !connectionId || value.connectionId === connectionId).slice(-limit).reverse(),
    clearCommandHistory: async () => { history = []; },
    search: async (query) => {
      const normalized = query.toLowerCase();
      return snapshot.connections
        .filter((value) => `${value.name} ${value.host} ${value.tags.join(" ")}`.toLowerCase().includes(normalized))
        .map((value) => ({ kind: "connection" as const, id: value.id, label: value.name, detail: value.host }));
    },
    loadLayout: async () => layout,
    saveLayout: async (value) => {
      layout = value;
    },
    exportWorkspace: async () => ({ fileName: "preview.firstshell-backup", bytes: 4096 }),
    validateImport: async () => ({ folders: 2, connections: 3, proxies: 0, terminalProfiles: 1, snippets: 1 }),
    restoreImport: async () => ({
      imported: { folders: 2, connections: 3, proxies: 0, terminalProfiles: 1, snippets: 1 },
      recoveryFileName: "before-import-preview.db",
    }),
    onVaultLocked: async () => () => undefined,
    configureCredential: async () => undefined,
    credentialStatus: async () => true,
    configureProxyCredential: async () => undefined,
    proxyCredentialStatus: async () => true,
    openTerminal: async (_connectionId, size, onEvent) => {
      const value: SessionSnapshot = { id: nextId("session"), state: "ready", size };
      sessions.set(value.id, value);
      queueMicrotask(() => onEvent({ sessionId: value.id, event: { type: "state", state: "ready" } }));
      return value;
    },
    openQuickTerminal: async (_input, size, onEvent) => {
      const value: SessionSnapshot = { id: nextId("quick-session"), state: "ready", size };
      sessions.set(value.id, value);
      queueMicrotask(() => onEvent({ sessionId: value.id, event: { type: "state", state: "ready" } }));
      return value;
    },
    listTerminals: async () => [...sessions.values()],
    sendTerminalInput: async (sessionId, data) => {
      if (!sessions.has(sessionId)) throw new Error("session not found");
      void data;
    },
    broadcastTerminal: async (sessionIds) => {
      if (sessionIds.some((id) => !sessions.has(id))) throw new Error("session not found");
    },
    stopBroadcast: async (sessionIds) => {
      if (sessionIds.some((id) => !sessions.has(id))) throw new Error("session not found");
    },
    resizeTerminal: async (sessionId, size) => {
      const value = sessions.get(sessionId);
      if (value) sessions.set(sessionId, { ...value, size });
    },
    reconnectTerminal: async (sessionId) => {
      const value = sessions.get(sessionId);
      if (value) sessions.set(sessionId, { ...value, state: "ready" });
    },
    closeTerminal: async (sessionId) => { sessions.delete(sessionId); },
    pendingHostKey: async () => null,
    approveHostKey: async () => undefined,
    startSessionLog: async (sessionId) => ({ path: `preview/session-logs/${sessionId}.log` }),
    stopSessionLog: async () => undefined,
    openSftp: async (connectionId) => ({ id: nextId("sftp"), connectionId, root: "/", state: "ready", hostKey: null }),
    approveSftpHostKey: async (workspaceId) => ({ id: workspaceId, connectionId: "connection-edge", root: "/", state: "ready", hostKey: null }),
    closeSftp: async () => undefined,
    listSftp: async (_workspaceId, path) => path ? [] : [
      { path: "logs", name: "logs", kind: "directory", size: null, modifiedAt: null, permissions: 0o755, linkTarget: null },
      { path: "README.txt", name: "README.txt", kind: "file", size: 1280, modifiedAt: null, permissions: 0o644, linkTarget: null },
    ],
    createSftpDirectory: async () => undefined,
    renameSftpEntry: async (_workspaceId, _path, name) => name,
    deleteSftpEntry: async () => undefined,
    chmodSftpEntry: async () => undefined,
    openRemoteText: async (_workspaceId, path) => ({ path, content: "Preview remote file\n", fingerprint: "preview-fingerprint", utf8Bom: false, bytes: 20 }),
    saveRemoteText: async (_workspaceId, path, _fingerprint, content, utf8Bom) => ({ path, content, fingerprint: `preview-${content.length}`, utf8Bom, bytes: new TextEncoder().encode(content).length }),
    createRemoteArchive: async (_workspaceId, _sources, destination) => ({ path: destination, bytes: 2048 }),
    chooseSftpUploads: async () => [],
    onSftpUploadDrop: async () => () => undefined,
    queueSftpDrop: async () => [],
    chooseSftpDownload: async () => null,
    chooseZmodemUpload: async () => null,
    chooseZmodemDownload: async () => null,
    listTransfers: async () => transfers,
    pauseTransfer: async (transferId) => { transfers = transfers.map((value) => value.id === transferId ? { ...value, status: "paused" } : value); },
    cancelTransfer: async (transferId) => { transfers = transfers.map((value) => value.id === transferId ? { ...value, status: "cancelled" } : value); },
    retryTransfer: async (transferId, onEvent) => {
      const value = transfers.find((item) => item.id === transferId);
      if (!value) throw new Error("transfer not found");
      const retried = { ...value, generation: value.generation + 1, status: "queued" as const, transferred: 0, error: null };
      transfers = transfers.map((item) => item.id === transferId ? retried : item);
      queueMicrotask(() => onEvent(retried));
      return retried;
    },
    clearFinishedTransfers: async () => { transfers = transfers.filter((value) => !["completed", "failed", "cancelled"].includes(value.status)); },
    startTunnel: async (connectionId, spec) => {
      const value: DesktopTunnelSnapshot = { id: nextId("tunnel"), connectionId, state: "running", tunnel: { ...spec, bindPort: spec.bindPort || 49152 }, hostKey: null };
      tunnels = [...tunnels, value];
      return value;
    },
    approveTunnelHostKey: async (tunnelId) => {
      const value = tunnels.find((item) => item.id === tunnelId);
      if (!value) throw new Error("tunnel not found");
      return value;
    },
    listTunnels: async () => tunnels,
    closeTunnel: async (tunnelId) => { tunnels = tunnels.filter((value) => value.id !== tunnelId); },
    startHostMonitor: async (connectionId, intervalSeconds, onEvent) => {
      const id = nextId("monitor");
      const sample: ProbeResult = { version: 1, probe: { type: "host_metrics" }, capturedAtMs: Date.now(), exitCode: 0, metrics: [{ key: "cpu_used", value: 24, unit: "percent" }, { key: "memory_total", value: 8_589_934_592, unit: "bytes" }, { key: "memory_available", value: 4_294_967_296, unit: "bytes" }, { key: "swap_total", value: 2_147_483_648, unit: "bytes" }, { key: "swap_free", value: 1_610_612_736, unit: "bytes" }, { key: "load_1m", value: 0.42, unit: "count" }], rows: [{ kind: "network", cells: ["eth0", "1048576", "524288"] }, { kind: "network", cells: ["lo", "4096", "4096"] }, { kind: "disk", cells: ["/", "107374182400", "64424509440"] }], diagnostic: null };
      monitors.set(id, [sample]);
      queueMicrotask(() => onEvent({ monitorId: id, state: "running", sample, diagnostic: null }));
      return { id, connectionId, intervalSeconds, state: "running", sampleCount: 1, diagnostic: null, hostKey: null };
    },
    approveMonitorHostKey: async (monitorId) => ({ id: monitorId, connectionId: "connection-edge", intervalSeconds: 5, state: "running", sampleCount: 0, diagnostic: null, hostKey: null }),
    monitorHistory: async (monitorId, maxPoints) => (monitors.get(monitorId) ?? []).slice(-maxPoints),
    runHostProbe: async (_connectionId, probe) => ({ version: 1, probe, capturedAtMs: Date.now(), exitCode: 0, metrics: probe.type === "host_metrics" ? [{ key: "cpu_used", value: 24, unit: "percent" }] : [], rows: probe.type === "processes" ? [{ kind: "process", cells: ["1234", "nginx", "1.2", "0.8"] }] : [{ kind: probe.type, cells: ["preview", "ok"] }], diagnostic: null }),
    stopHostMonitor: async (monitorId) => { monitors.delete(monitorId); },
  };
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export const workspaceBridge: WorkspaceBridge = window.__TAURI_INTERNALS__
  ? tauriBridge
  : createPreviewBridge();
