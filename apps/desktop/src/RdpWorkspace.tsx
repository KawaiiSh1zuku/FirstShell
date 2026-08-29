import { KeyRound, MonitorUp, RefreshCw, Unplug } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { WorkspaceBridge } from "./bridge";
import { CredentialDialog } from "./CredentialDialog";
import type { ConnectionProfile, DesktopRdpEvent, RdpHelperState } from "./types";

export function RdpWorkspace({ bridge, connections, activeConnectionId, onActivity }: {
  bridge: WorkspaceBridge;
  connections: ConnectionProfile[];
  activeConnectionId: string | null;
  onActivity(message: string): void;
}) {
  return (
    <div className="rdp-tabs-content">
      {connections.map((connection) => (
        <RdpConnection
          key={connection.id}
          bridge={bridge}
          connection={connection}
          active={connection.id === activeConnectionId}
          onActivity={onActivity}
        />
      ))}
    </div>
  );
}

function RdpConnection({ bridge, connection, active, onActivity }: {
  bridge: WorkspaceBridge;
  connection: ConnectionProfile;
  active: boolean;
  onActivity(message: string): void;
}) {
  const containerRef = useRef<HTMLElement>(null);
  const sessionIdRef = useRef<string | null>(null);
  const generationRef = useRef(0);
  const [state, setState] = useState<RdpHelperState>("starting");
  const [error, setError] = useState<string | null>(null);
  const [credentialOpen, setCredentialOpen] = useState(false);

  const handleEvent = useCallback((envelope: DesktopRdpEvent) => {
    if (sessionIdRef.current && envelope.sessionId !== sessionIdRef.current) return;
    sessionIdRef.current ??= envelope.sessionId;
    if (envelope.event.type === "state") {
      setState(envelope.event.payload.state);
      return;
    }
    if (envelope.event.type === "exited") {
      sessionIdRef.current = null;
      setState(envelope.event.payload.requested ? "stopped" : "failed");
      if (!envelope.event.payload.requested) setError("RDP 后端已退出，可重新连接");
      return;
    }
    if (envelope.event.type === "error") {
      sessionIdRef.current = null;
      setState("failed");
      setError(envelope.event.payload.message);
    }
  }, []);

  const disconnect = useCallback(async () => {
    generationRef.current += 1;
    const sessionId = sessionIdRef.current;
    sessionIdRef.current = null;
    if (sessionId) await bridge.closeRdp(sessionId).catch(() => undefined);
    setState("stopped");
  }, [bridge]);

  const connect = useCallback(async () => {
    if (sessionIdRef.current) return;
    const generation = ++generationRef.current;
    setState("starting");
    setError(null);
    const rect = containerRef.current?.getBoundingClientRect();
    try {
      const session = await bridge.openRdp(
        connection.id,
        {
          width: Math.max(640, Math.min(65535, Math.round(rect?.width ?? 1280))),
          height: Math.max(480, Math.min(65535, Math.round(rect?.height ?? 720))),
        },
        handleEvent,
      );
      if (generation !== generationRef.current) {
        await bridge.closeRdp(session.id).catch(() => undefined);
        return;
      }
      if (!sessionIdRef.current) {
        sessionIdRef.current = session.id;
        setState(session.state);
      }
      onActivity(`${connection.name} RDP helper 已创建`);
    } catch (reason) {
      if (generation !== generationRef.current) return;
      const message = errorMessage(reason);
      setState("failed");
      setError(message);
      if (message.includes("credential") || message.includes("凭据")) setCredentialOpen(true);
    }
  }, [bridge, connection.id, connection.name, handleEvent, onActivity]);

  useEffect(() => {
    void connect();
    return () => {
      generationRef.current += 1;
      const sessionId = sessionIdRef.current;
      sessionIdRef.current = null;
      if (sessionId) void bridge.closeRdp(sessionId).catch(() => undefined);
    };
  }, [bridge, connect]);

  return (
    <section ref={containerRef} className={`rdp-connection ${active ? "active" : ""}`} aria-hidden={!active}>
      <header className="terminal-toolbar">
        <div className="terminal-endpoint"><strong>{connection.name}</strong><span>{connection.username ? `${connection.username}@` : ""}{connection.host}:{connection.port}</span></div>
        <div className="terminal-tools">
          <span className={`rdp-state ${state}`}>{stateLabel(state)}</span>
          <button className="icon-button" type="button" aria-label="更新 RDP 凭据" title="更新凭据" onClick={() => setCredentialOpen(true)}><KeyRound size={15} /></button>
          <button className="icon-button" type="button" aria-label="重新连接 RDP" title="重新连接" disabled={state === "starting" || state === "running"} onClick={() => void connect()}><RefreshCw size={15} /></button>
          <button className="icon-button" type="button" aria-label="断开 RDP" title="断开" disabled={!sessionIdRef.current} onClick={() => void disconnect()}><Unplug size={15} /></button>
        </div>
      </header>
      <div className="rdp-companion-status">
        <MonitorUp size={36} />
        <strong>{state === "running" ? "远程桌面已在伴随窗口中运行" : stateLabel(state)}</strong>
        {error && <p role="alert">{error}</p>}
      </div>
      {credentialOpen && <CredentialDialog title={`${connection.name} 的 RDP 凭据`} description="凭据由 Rust 写入操作系统凭据库，连接时单次传给隔离 helper。" label="RDP 密码" persistent onClose={() => setCredentialOpen(false)} onSubmit={async (secret) => { await bridge.configureRdpCredential(connection.id, secret); await disconnect(); await connect(); }} />}
    </section>
  );
}

function stateLabel(state: RdpHelperState) {
  if (state === "starting") return "正在启动";
  if (state === "running") return "已连接";
  if (state === "stopped") return "已断开";
  return "连接失败";
}

function errorMessage(reason: unknown) {
  return reason instanceof Error
    ? reason.message
    : typeof reason === "object" && reason && "message" in reason
      ? String(reason.message)
      : "RDP 会话创建失败";
}
