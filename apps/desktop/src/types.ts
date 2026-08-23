export type ConnectionKind = "ssh" | "rdp" | "local";
export type ProxyKind = "direct" | "socks5" | "http_connect" | "system";

export interface VaultStatus {
  initialized: boolean;
  unlocked: boolean;
  initializedThisRun: boolean;
  keyringAvailable: boolean;
  idleTimeoutSeconds: number;
}

export interface Folder {
  id: string;
  parentId: string | null;
  revision: number;
  sortOrder: number;
  name: string;
  notes: string;
}

export interface FolderInput {
  parentId: string | null;
  sortOrder: number;
  name: string;
  notes: string;
}

export interface ConnectionProfile {
  id: string;
  folderId: string | null;
  revision: number;
  sortOrder: number;
  kind: ConnectionKind;
  name: string;
  host: string;
  port: number;
  username: string;
  proxyId: string | null;
  terminalProfileId: string | null;
  tags: string[];
  notes: string;
}

export interface ConnectionInput {
  folderId: string | null;
  sortOrder: number;
  kind: ConnectionKind;
  name: string;
  host: string;
  port: number;
  username: string;
  proxyId: string | null;
  terminalProfileId: string | null;
  tags: string[];
  notes: string;
}

export interface ProxyProfile {
  id: string;
  revision: number;
  kind: ProxyKind;
  name: string;
  host: string;
  port: number;
  username: string;
}

export interface ProxyInput {
  kind: ProxyKind;
  name: string;
  host: string;
  port: number;
  username: string;
}

export interface TerminalProfile {
  id: string;
  revision: number;
  name: string;
  fontFamily: string;
  fontSize: number;
  theme: string;
  scrollback: number;
}

export interface TerminalProfileInput {
  name: string;
  fontFamily: string;
  fontSize: number;
  theme: string;
  scrollback: number;
}

export interface WorkspaceSnapshot {
  folders: Folder[];
  connections: ConnectionProfile[];
  proxies: ProxyProfile[];
  terminalProfiles: TerminalProfile[];
}

export interface WorkspaceLayout {
  leftWidth: number;
  rightWidth: number;
  bottomHeight: number;
  bottomOpen: boolean;
}

export interface SearchResult {
  kind: "folder" | "connection" | "proxy" | "terminal_profile";
  id: string;
  label: string;
  detail: string;
}

export interface ExportReceipt {
  fileName: string;
  bytes: number;
}

export interface ImportSummary {
  folders: number;
  connections: number;
  proxies: number;
  terminalProfiles: number;
}

export interface RestoreReceipt {
  imported: ImportSummary;
  recoveryFileName: string;
}

export interface PublicError {
  code: string;
  message: string;
  correlationId: string;
}

export type Selection =
  | { kind: "folder"; id: string }
  | { kind: "connection"; id: string }
  | null;
