import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  ConnectionInput,
  ConnectionProfile,
  ExportReceipt,
  Folder,
  FolderInput,
  ImportSummary,
  ProxyInput,
  ProxyProfile,
  RestoreReceipt,
  SearchResult,
  TerminalProfile,
  TerminalProfileInput,
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
  search(query: string): Promise<SearchResult[]>;
  loadLayout(): Promise<WorkspaceLayout>;
  saveLayout(layout: WorkspaceLayout): Promise<void>;
  exportWorkspace(password: string): Promise<ExportReceipt>;
  validateImport(fileName: string, password: string): Promise<ImportSummary>;
  restoreImport(fileName: string, password: string): Promise<RestoreReceipt>;
  onVaultLocked(callback: () => void): Promise<() => void>;
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
  search: (query) => invoke("search_workspace", { query, limit: 50 }),
  loadLayout: () => invoke("load_workspace_layout"),
  saveLayout: (layout) => invoke("save_workspace_layout", { layout }),
  exportWorkspace: (password) => invoke("export_workspace", { password }),
  validateImport: (fileName, password) =>
    invoke("validate_workspace_import", { fileName, password }),
  restoreImport: (fileName, password) =>
    invoke("restore_workspace_import", { fileName, password }),
  onVaultLocked: async (callback) => listen("vault-locked", callback),
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
      terminalProfileId: "terminal-default",
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
      terminalProfileId: "terminal-default",
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
      terminalProfileId: null,
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
      scrollback: 10000,
    },
  ],
};

export function createPreviewBridge(): WorkspaceBridge {
  let snapshot = structuredClone(previewSnapshot);
  const deletedFolders = new Map<string, Folder>();
  const deletedConnections = new Map<string, ConnectionProfile>();
  let layout: WorkspaceLayout = {
    leftWidth: 256,
    rightWidth: 288,
    bottomHeight: 180,
    bottomOpen: true,
  };
  let sequence = 1;
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
    validateImport: async () => ({ folders: 2, connections: 3, proxies: 0, terminalProfiles: 1 }),
    restoreImport: async () => ({
      imported: { folders: 2, connections: 3, proxies: 0, terminalProfiles: 1 },
      recoveryFileName: "before-import-preview.db",
    }),
    onVaultLocked: async () => () => undefined,
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
