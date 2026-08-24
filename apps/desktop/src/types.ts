export type ConnectionKind = "ssh" | "rdp" | "local";
export type ProxyKind = "direct" | "socks5" | "http_connect" | "system";
export type SshAuthenticationKind = "agent" | "password" | "private_key" | "keyboard_interactive";
export type TerminalEncoding = "utf8" | "gb18030" | "big5";
export type TerminalKeymap = "xterm" | "windows" | "emacs";

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
  authentication: SshAuthenticationKind;
  privateKeyPath: string;
  allowLegacyAlgorithms: boolean;
  opensshFallback: boolean;
  tags: string[];
  notes: string;
  /** Frontend-only marker for a quick connection that is never persisted. */
  temporary?: boolean;
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
  authentication: SshAuthenticationKind;
  privateKeyPath: string;
  allowLegacyAlgorithms: boolean;
  opensshFallback: boolean;
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
  encoding: TerminalEncoding;
  keymap: TerminalKeymap;
}

export interface TerminalProfileInput {
  name: string;
  fontFamily: string;
  fontSize: number;
  theme: string;
  scrollback: number;
  encoding: TerminalEncoding;
  keymap: TerminalKeymap;
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

export type SessionState = "connecting" | "authenticating" | "ready" | "disconnected" | "reconnecting" | "closing" | "closed";

export interface TerminalSize {
  cols: number;
  rows: number;
  pixelWidth: number;
  pixelHeight: number;
}

export interface SessionSnapshot {
  id: string;
  state: SessionState;
  size: TerminalSize;
}

export type SessionEvent =
  | { type: "state"; state: SessionState }
  | { type: "output"; data: number[] }
  | { type: "banner"; text: string }
  | { type: "exit"; code: number | null }
  | { type: "error"; code: string; message: string };

export interface DesktopSessionEvent {
  sessionId: string;
  event: SessionEvent;
}

export interface HostKeyEvidence {
  host: string;
  port: number;
  algorithm: string;
  fingerprint: string;
  changed: boolean;
  previousFingerprint: string | null;
}

export interface SessionLogReceipt {
  path: string;
}

export interface QuickConnectInput {
  host: string;
  port: number;
  username: string;
  authentication: SshAuthenticationKind;
  privateKeyPath: string;
  allowLegacyAlgorithms: boolean;
  opensshFallback: boolean;
}

export type Selection =
  | { kind: "folder"; id: string }
  | { kind: "connection"; id: string }
  | null;
