export type ConnectionKind = "ssh" | "rdp" | "local";
export type ProxyKind = "direct" | "socks5" | "http_connect" | "system";
export type ProxyDnsMode = "remote" | "local";
export type SshAuthenticationKind = "agent" | "password" | "private_key" | "keyboard_interactive";
export type TerminalEncoding = "utf8" | "gb18030" | "big5";
export type TerminalKeymap = "xterm" | "windows" | "emacs";

export interface TerminalPalette {
  foreground: string;
  background: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

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
  jumpHostIds: string[];
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
  jumpHostIds: string[];
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
  dnsMode: ProxyDnsMode;
}

export interface ProxyInput {
  kind: ProxyKind;
  name: string;
  host: string;
  port: number;
  username: string;
  dnsMode: ProxyDnsMode;
}

export interface TerminalProfile {
  id: string;
  revision: number;
  name: string;
  fontFamily: string;
  fontSize: number;
  theme: string;
  palette: TerminalPalette | null;
  scrollback: number;
  encoding: TerminalEncoding;
  keymap: TerminalKeymap;
}

export interface TerminalProfileInput {
  name: string;
  fontFamily: string;
  fontSize: number;
  theme: string;
  palette: TerminalPalette | null;
  scrollback: number;
  encoding: TerminalEncoding;
  keymap: TerminalKeymap;
}

export interface WorkspaceSnapshot {
  folders: Folder[];
  connections: ConnectionProfile[];
  proxies: ProxyProfile[];
  terminalProfiles: TerminalProfile[];
  snippets: Snippet[];
}

export interface WorkspaceLayout {
  leftWidth: number;
  leftOpen: boolean;
  rightWidth: number;
  bottomHeight: number;
  bottomOpen: boolean;
}

export interface SearchResult {
  kind: "folder" | "connection" | "proxy" | "terminal_profile" | "snippet";
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
  snippets: number;
}

export type SnippetParameterKind = "text" | "integer" | "choice" | "host" | "path";

export interface SnippetParameter {
  name: string;
  label: string;
  kind: SnippetParameterKind;
  required: boolean;
  defaultValue: string;
  choices: string[];
}

export interface Snippet {
  id: string;
  revision: number;
  name: string;
  tags: string[];
  template: string;
  parameters: SnippetParameter[];
}

export interface SnippetInput {
  name: string;
  tags: string[];
  template: string;
  parameters: SnippetParameter[];
}

export interface CommandHistoryEntry {
  connectionId: string;
  command: string;
  path: string | null;
  recordedAtMs: number;
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

export interface ReadyTerminalSession {
  sessionId: string;
  connectionId: string;
}

export interface TerminalInsertRequest {
  sequence: number;
  connectionId: string;
  text: string;
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

export type RemoteEntryKind = "file" | "directory" | "symlink" | "other";

export interface RemoteEntry {
  path: string;
  name: string;
  kind: RemoteEntryKind;
  size: number | null;
  modifiedAt: number | null;
  permissions: number | null;
  linkTarget: string | null;
}

export interface RemoteEditDocument {
  path: string;
  content: string;
  fingerprint: string;
  utf8Bom: boolean;
  bytes: number;
}

export interface ArchiveReceipt {
  path: string;
  bytes: number;
}

export interface SftpWorkspaceSnapshot {
  id: string;
  connectionId: string;
  root: string;
  state: "host_key_required" | "ready";
  hostKey: HostKeyEvidence | null;
}

export type ConflictPolicy = "deny" | "replace" | "resume";
export type VerifyMode = "size" | "sha256";

export interface TransferOptions {
  conflict: ConflictPolicy;
  verify: VerifyMode;
}

export interface StagedUploadDrop {
  batchId: string | null;
  files: { name: string; size: number }[];
  error: string | null;
}

export type TransferStatus = "queued" | "running" | "pausing" | "cancelling" | "paused" | "completed" | "failed" | "cancelled";

export interface TransferSnapshot {
  id: string;
  workspaceId: string;
  generation: number;
  direction: "upload" | "download";
  name: string;
  remotePath: string;
  status: TransferStatus;
  transferred: number;
  total: number;
  resumed: boolean;
  sha256: string | null;
  error: string | null;
}

export type TunnelKind = "local" | "remote" | "dynamic";

export interface TunnelSpec {
  kind: TunnelKind;
  bindHost: string;
  bindPort: number;
  targetHost: string;
  targetPort: number;
  allowNonLoopback: boolean;
}

export interface TunnelSnapshot {
  kind: TunnelKind;
  bindHost: string;
  bindPort: number;
  targetHost: string;
  targetPort: number;
}

export interface DesktopTunnelSnapshot {
  id: string;
  connectionId: string;
  state: "host_key_required" | "running";
  tunnel: TunnelSnapshot | null;
  hostKey: HostKeyEvidence | null;
}

export type ProbeKind =
  | { type: "host_metrics" }
  | { type: "processes" }
  | { type: "listening_ports" }
  | { type: "ping"; target: string; count: number }
  | { type: "traceroute"; target: string; max_hops: number };

export interface ProbeMetric {
  key: string;
  value: number;
  unit: string;
}

export interface ProbeRow {
  kind: string;
  cells: string[];
}

export interface ProbeResult {
  version: number;
  probe: ProbeKind;
  capturedAtMs: number;
  exitCode: number | null;
  metrics: ProbeMetric[];
  rows: ProbeRow[];
  diagnostic: string | null;
}

export type MonitorState = "host_key_required" | "running" | "degraded";

export interface MonitorSnapshot {
  id: string;
  connectionId: string;
  intervalSeconds: number;
  state: MonitorState;
  sampleCount: number;
  diagnostic: string | null;
  hostKey: HostKeyEvidence | null;
}

export interface MonitorEvent {
  monitorId: string;
  state: MonitorState;
  sample: ProbeResult | null;
  diagnostic: string | null;
}

export type Selection =
  | { kind: "folder"; id: string }
  | { kind: "connection"; id: string }
  | null;
