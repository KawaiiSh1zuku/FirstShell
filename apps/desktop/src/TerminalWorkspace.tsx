import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { Columns2, FileText, KeyRound, Play, Rows2, ScrollText, Unplug, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { WorkspaceBridge } from "./bridge";
import type {
  ConnectionProfile,
  DesktopSessionEvent,
  HostKeyEvidence,
  SessionState,
  TerminalProfile,
  TerminalSize,
} from "./types";

interface TerminalWorkspaceProps {
  bridge: WorkspaceBridge;
  connections: ConnectionProfile[];
  activeConnectionId: string | null;
  profiles: TerminalProfile[];
  onActivity(message: string): void;
  onReadyCountChange(count: number): void;
}

interface PaneModel {
  id: number;
}

let nextPaneId = 1;

export function TerminalWorkspace(props: TerminalWorkspaceProps) {
  const [readyCounts, setReadyCounts] = useState<Record<string, number>>({});
  const connectionKey = props.connections.map((connection) => connection.id).join("\u0000");
  const reportReady = useCallback((connectionId: string, count: number) => {
    setReadyCounts((current) => current[connectionId] === count ? current : { ...current, [connectionId]: count });
  }, []);
  useEffect(() => {
    const visible = new Set(props.connections.map((connection) => connection.id));
    setReadyCounts((current) => Object.fromEntries(Object.entries(current).filter(([id]) => visible.has(id))));
  }, [connectionKey]);
  useEffect(() => {
    props.onReadyCountChange(Object.values(readyCounts).reduce((total, value) => total + value, 0));
  }, [props.onReadyCountChange, readyCounts]);
  return (
    <div className="terminal-tabs-content">
      {props.connections.map((connection) => (
        <ConnectionTerminal
          key={connection.id}
          bridge={props.bridge}
          connection={connection}
          profile={props.profiles.find((value) => value.id === connection.terminalProfileId)}
          active={connection.id === props.activeConnectionId}
          onActivity={props.onActivity}
          onReadyCountChange={(count) => reportReady(connection.id, count)}
        />
      ))}
    </div>
  );
}

function ConnectionTerminal({ bridge, connection, profile, active, onActivity, onReadyCountChange }: {
  bridge: WorkspaceBridge;
  connection: ConnectionProfile;
  profile?: TerminalProfile;
  active: boolean;
  onActivity(message: string): void;
  onReadyCountChange(count: number): void;
}) {
  const [panes, setPanes] = useState<PaneModel[]>(() => [{ id: nextPaneId++ }]);
  const [direction, setDirection] = useState<"columns" | "rows">("columns");
  const [activePane, setActivePane] = useState(panes[0].id);
  const [readyPanes, setReadyPanes] = useState<Set<number>>(() => new Set());
  const updatePaneReady = useCallback((paneId: number, ready: boolean) => {
    setReadyPanes((current) => {
      const next = new Set(current);
      if (ready) next.add(paneId); else next.delete(paneId);
      return next;
    });
  }, []);

  useEffect(() => {
    onReadyCountChange(readyPanes.size);
  }, [onReadyCountChange, readyPanes]);

  const addPane = (nextDirection: "columns" | "rows") => {
    const pane = { id: nextPaneId++ };
    setDirection(nextDirection);
    setPanes((current) => [...current, pane].slice(0, 4));
    setActivePane(pane.id);
  };

  const closePane = (id: number) => {
    setPanes((current) => {
      if (current.length === 1) return current;
      const next = current.filter((pane) => pane.id !== id);
      setActivePane((selected) => selected === id ? next[0].id : selected);
      return next;
    });
    setReadyPanes((current) => {
      const next = new Set(current);
      next.delete(id);
      return next;
    });
  };

  return (
    <section className={`connection-terminal ${active ? "active" : ""}`} aria-hidden={!active}>
      <header className="terminal-toolbar">
        <div className="terminal-endpoint"><strong>{connection.name}</strong><span>{connection.kind === "local" ? "Local" : `${connection.username ? `${connection.username}@` : ""}${connection.host}:${connection.port}`}</span></div>
        <div className="terminal-tools">
          <button className="icon-button" type="button" aria-label="左右分屏" title="左右分屏" disabled={panes.length >= 4} onClick={() => addPane("columns")}><Columns2 size={15} /></button>
          <button className="icon-button" type="button" aria-label="上下分屏" title="上下分屏" disabled={panes.length >= 4} onClick={() => addPane("rows")}><Rows2 size={15} /></button>
        </div>
      </header>
      <div className={`terminal-grid ${direction}`}>
        {panes.map((pane) => (
          <TerminalPane
            key={pane.id}
            bridge={bridge}
            paneId={pane.id}
            connection={connection}
            profile={profile}
            active={active && activePane === pane.id}
            canClose={panes.length > 1}
            onActivate={() => setActivePane(pane.id)}
            onClose={() => closePane(pane.id)}
            onActivity={onActivity}
            onReadyChange={updatePaneReady}
          />
        ))}
      </div>
    </section>
  );
}

function TerminalPane({ bridge, paneId, connection, profile, active, canClose, onActivate, onClose, onActivity, onReadyChange }: {
  bridge: WorkspaceBridge;
  paneId: number;
  connection: ConnectionProfile;
  profile?: TerminalProfile;
  active: boolean;
  canClose: boolean;
  onActivate(): void;
  onClose(): void;
  onActivity(message: string): void;
  onReadyChange(paneId: number, ready: boolean): void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const lifecycleRef = useRef(0);
  const [state, setState] = useState<SessionState>("connecting");
  const [error, setError] = useState<string | null>(null);
  const [hostKey, setHostKey] = useState<HostKeyEvidence | null>(null);
  const [logPath, setLogPath] = useState<string | null>(null);
  const [credentialBusy, setCredentialBusy] = useState(false);
  const textEncoder = useMemo(() => new TextEncoder(), []);

  const terminalSize = useCallback((): TerminalSize => {
    const terminal = terminalRef.current;
    const container = containerRef.current;
    return {
      cols: Math.max(2, terminal?.cols ?? 100),
      rows: Math.max(1, terminal?.rows ?? 30),
      pixelWidth: Math.min(65535, Math.max(0, Math.round(container?.clientWidth ?? 0))),
      pixelHeight: Math.min(65535, Math.max(0, Math.round(container?.clientHeight ?? 0))),
    };
  }, []);

  const handleEvent = useCallback((envelope: DesktopSessionEvent) => {
    if (envelope.event.type === "state") {
      setState(envelope.event.state);
      onReadyChange(paneId, envelope.event.state === "ready");
      return;
    }
    if (envelope.event.type === "output") {
      terminalRef.current?.write(Uint8Array.from(envelope.event.data));
      return;
    }
    if (envelope.event.type === "banner") {
      terminalRef.current?.writeln(`\r\n${envelope.event.text}\r\n`);
      return;
    }
    if (envelope.event.type === "exit") {
      terminalRef.current?.writeln(`\r\n[session exited${envelope.event.code === null ? "" : `: ${envelope.event.code}`}]`);
      return;
    }
    setError(envelope.event.message);
    terminalRef.current?.writeln(`\r\n[${envelope.event.code}] ${envelope.event.message}`);
    if (envelope.event.code === "HOST_KEY_UNKNOWN" || envelope.event.code === "HOST_KEY_CHANGED") {
      void bridge.pendingHostKey(envelope.sessionId).then(setHostKey).catch(() => undefined);
    }
  }, [bridge, onReadyChange, paneId]);

  const connect = useCallback(async (generation = lifecycleRef.current) => {
    if (sessionIdRef.current) return;
    setError(null);
    setHostKey(null);
    setState("connecting");
    try {
      const session = connection.temporary
        ? await bridge.openQuickTerminal({
            host: connection.host,
            port: connection.port,
            username: connection.username,
            authentication: connection.authentication,
            privateKeyPath: connection.privateKeyPath,
            allowLegacyAlgorithms: connection.allowLegacyAlgorithms,
            opensshFallback: connection.opensshFallback,
          }, terminalSize(), handleEvent)
        : await bridge.openTerminal(connection.id, terminalSize(), handleEvent);
      if (generation !== lifecycleRef.current || !terminalRef.current) {
        await bridge.closeTerminal(session.id).catch(() => undefined);
        return;
      }
      if (sessionIdRef.current) {
        await bridge.closeTerminal(session.id).catch(() => undefined);
        return;
      }
      sessionIdRef.current = session.id;
      setState(session.state);
      onActivity(`${connection.name} 会话已创建`);
    } catch (reason) {
      if (generation !== lifecycleRef.current) return;
      const message = reason instanceof Error ? reason.message : typeof reason === "object" && reason && "message" in reason ? String(reason.message) : "会话创建失败";
      setError(message);
      setState("disconnected");
    }
  }, [bridge, connection, handleEvent, onActivity, terminalSize]);

  useEffect(() => {
    const generation = ++lifecycleRef.current;
    const container = containerRef.current;
    if (!container) return;
    const terminal = new Terminal({
      allowProposedApi: false,
      cursorBlink: true,
      cursorStyle: "block",
      fontFamily: profile?.fontFamily ?? "Cascadia Mono, Consolas, monospace",
      fontSize: profile?.fontSize ?? 14,
      scrollback: profile?.scrollback ?? 10_000,
      theme: terminalTheme(profile?.theme),
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(container);
    fit.fit();
    terminal.focus();
    terminalRef.current = terminal;
    fitRef.current = fit;
    const input = terminal.onData((data) => {
      const sessionId = sessionIdRef.current;
      if (sessionId) void bridge.sendTerminalInput(sessionId, [...textEncoder.encode(data)]).catch(() => undefined);
    });
    const resize = new ResizeObserver(() => {
      fit.fit();
      const sessionId = sessionIdRef.current;
      if (sessionId) void bridge.resizeTerminal(sessionId, terminalSize()).catch(() => undefined);
    });
    resize.observe(container);
    void connect(generation);
    return () => {
      lifecycleRef.current += 1;
      resize.disconnect();
      input.dispose();
      terminal.dispose();
      terminalRef.current = null;
      fitRef.current = null;
      const sessionId = sessionIdRef.current;
      sessionIdRef.current = null;
      if (sessionId) void bridge.closeTerminal(sessionId).catch(() => undefined);
      onReadyChange(paneId, false);
    };
  }, [bridge, connect, paneId, profile?.fontFamily, profile?.fontSize, profile?.scrollback, profile?.theme, terminalSize, textEncoder, onReadyChange]);

  useEffect(() => {
    if (!active) return;
    fitRef.current?.fit();
    terminalRef.current?.focus();
  }, [active]);

  const reconnect = async () => {
    const sessionId = sessionIdRef.current;
    if (!sessionId) {
      await connect();
      return;
    }
    setError(null);
    await bridge.reconnectTerminal(sessionId);
  };

  const approve = async () => {
    const sessionId = sessionIdRef.current;
    if (!sessionId || !hostKey) return;
    await bridge.approveHostKey(sessionId, hostKey.fingerprint, hostKey.previousFingerprint);
    setHostKey(null);
    setError(null);
  };

  const configureCredential = async () => {
    setCredentialBusy(true);
    try {
      if (await bridge.configureCredential(connection.id)) {
        onActivity(`${connection.name} 凭据已写入系统密钥环`);
        await reconnect();
      }
    } finally {
      setCredentialBusy(false);
    }
  };

  const toggleLog = async () => {
    const sessionId = sessionIdRef.current;
    if (!sessionId) return;
    if (logPath) {
      await bridge.stopSessionLog(sessionId);
      setLogPath(null);
      onActivity(`${connection.name} 会话日志已停止`);
    } else {
      const receipt = await bridge.startSessionLog(sessionId);
      setLogPath(receipt.path);
      onActivity(`${connection.name} 会话日志已启用`);
    }
  };

  const needsCredential = connection.kind === "ssh"
    && (connection.authentication === "password" || connection.authentication === "keyboard_interactive")
    && error?.includes("credential");

  return (
    <div className={`terminal-pane ${active ? "active" : ""}`} onMouseDown={onActivate}>
      <div className="pane-statusbar">
        <span className={`session-state ${state}`}>{stateLabel(state)}</span>
        <span className="pane-meta">{profile?.encoding?.toUpperCase() ?? "UTF8"}</span>
        {logPath && <span className="log-path" title={logPath}>{logPath}</span>}
        <button className="icon-button" type="button" aria-label={logPath ? "停止会话日志" : "开启会话日志"} title={logPath ? "停止日志" : "开启日志"} disabled={!sessionIdRef.current} onClick={() => void toggleLog()}><ScrollText size={14} /></button>
        <button className="icon-button" type="button" aria-label="重新连接" title="重新连接" disabled={state !== "disconnected"} onClick={() => void reconnect()}><Play size={14} /></button>
        {canClose && <button className="icon-button" type="button" aria-label="关闭分屏" title="关闭分屏" onClick={onClose}><X size={14} /></button>}
      </div>
      <div className="xterm-host" ref={containerRef} />
      {hostKey && <div className={`host-key-prompt ${hostKey.changed ? "changed" : ""}`} role="alertdialog" aria-label="主机密钥确认">
        <KeyRound size={18} />
        <div><strong>{hostKey.changed ? "主机密钥已变化" : "首次连接确认"}</strong><span>{hostKey.host}:{hostKey.port} · {hostKey.algorithm}</span>{hostKey.previousFingerprint && <code>{hostKey.previousFingerprint}</code>}<code>{hostKey.fingerprint}</code></div>
        <button type="button" onClick={() => void approve()}>{hostKey.changed ? "替换并重连" : "信任并重连"}</button>
      </div>}
      {error && !hostKey && <div className="terminal-error" role="alert"><Unplug size={17} /><span>{error}</span>{needsCredential && <button type="button" disabled={credentialBusy} onClick={() => void configureCredential()}><KeyRound size={14} />{credentialBusy ? "等待输入" : "配置凭据"}</button>}<button type="button" onClick={() => void reconnect()}><Play size={14} />重连</button></div>}
      {logPath && <div className="logging-indicator"><FileText size={12} />记录中</div>}
    </div>
  );
}

function terminalTheme(theme: string | undefined) {
  if (theme === "firstshell-light") return { background: "#f7f9f7", foreground: "#243128", cursor: "#167447", selectionBackground: "#b9dfc8" };
  if (theme === "high-contrast") return { background: "#000000", foreground: "#ffffff", cursor: "#00ff7f", selectionBackground: "#295f3f" };
  return { background: "#0e100e", foreground: "#d7ddd8", cursor: "#4fd18a", selectionBackground: "#284734" };
}

function stateLabel(state: SessionState) {
  const labels: Record<SessionState, string> = {
    connecting: "连接中",
    authenticating: "认证中",
    ready: "已连接",
    disconnected: "已断开",
    reconnecting: "重连中",
    closing: "关闭中",
    closed: "已关闭",
  };
  return labels[state];
}
