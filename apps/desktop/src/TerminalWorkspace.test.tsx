import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { TerminalWorkspace } from "./TerminalWorkspace";
import type { ConnectionProfile, DesktopSessionEvent, HostKeyEvidence, SessionSnapshot, TerminalProfile, TerminalSize } from "./types";
import { DEFAULT_CUSTOM_PALETTE } from "./terminalThemes";

const xtermMock = vi.hoisted(() => {
  const terminalInstances: Array<{ dispose: ReturnType<typeof vi.fn>; dataCallback?: (data: string) => void }> = [];
  const terminalOptions: unknown[] = [];
  const fitInstances: Array<{ fit: ReturnType<typeof vi.fn> }> = [];
  class MockTerminal {
    cols = 100;
    rows = 30;
    write = vi.fn();
    writeln = vi.fn();
    focus = vi.fn();
    dispose = vi.fn();
    loadAddon = vi.fn();
    open = vi.fn();
    dataCallback?: (data: string) => void;
    onData = vi.fn((callback: (data: string) => void) => { this.dataCallback = callback; return { dispose: vi.fn() }; });
    constructor(options: unknown) { terminalInstances.push(this); terminalOptions.push(options); }
  }
  return { fitInstances, terminalInstances, terminalOptions, MockTerminal };
});

vi.mock("@xterm/xterm", () => ({ Terminal: xtermMock.MockTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn(); constructor() { xtermMock.fitInstances.push(this); } } }));

class MockResizeObserver {
  observe() {}
  disconnect() {}
}

const connection: ConnectionProfile = {
  id: "connection-test",
  folderId: null,
  revision: 1,
  sortOrder: 0,
  kind: "ssh",
  name: "Test SSH",
  host: "server.test",
  port: 22,
  username: "operator",
  proxyId: null,
  jumpHostIds: [],
  terminalProfileId: "terminal-test",
  authentication: "agent",
  privateKeyPath: "",
  allowLegacyAlgorithms: false,
  opensshFallback: false,
  tags: [],
  notes: "",
};

const profile: TerminalProfile = {
  id: "terminal-test",
  revision: 1,
  name: "Default",
  fontFamily: "Cascadia Mono",
  fontSize: 14,
  theme: "firstshell-dark",
  palette: null,
  scrollback: 10000,
  encoding: "utf8",
  keymap: "xterm",
};

afterEach(() => {
  cleanup();
  xtermMock.fitInstances.length = 0;
  xtermMock.terminalInstances.length = 0;
  xtermMock.terminalOptions.length = 0;
  vi.restoreAllMocks();
});

describe("M2 terminal workspace", () => {
  it("closes stale sessions without accepting their late closed events", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const pending: Array<{
      onEvent(event: DesktopSessionEvent): void;
      resolve(session: SessionSnapshot): void;
      size: TerminalSize;
    }> = [];
    const open = vi.fn((_connectionId: string, size: TerminalSize, onEvent: (event: DesktopSessionEvent) => void) => new Promise<SessionSnapshot>((resolve) => pending.push({ onEvent, resolve, size })));
    bridge.openTerminal = open;
    const close = vi.spyOn(bridge, "closeTerminal");
    const activity = vi.fn();
    const readyConnections = vi.fn();
    render(<StrictMode><TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={activity} onReadySessionsChange={() => undefined} onReadyConnectionsChange={readyConnections} insertRequest={null} /></StrictMode>);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));

    pending[1].resolve({ id: "session-current", state: "ready", size: pending[1].size });
    expect(await screen.findByText("已连接")).toBeTruthy();
    await waitFor(() => expect(readyConnections).toHaveBeenLastCalledWith([connection.id]));

    pending[0].resolve({ id: "session-stale", state: "ready", size: pending[0].size });
    await waitFor(() => expect(close).toHaveBeenCalledWith("session-stale"));
    const callCount = readyConnections.mock.calls.length;
    pending[0].onEvent({ sessionId: "session-stale", event: { type: "state", state: "closed" } });
    await waitFor(() => expect(screen.getByText("已连接")).toBeTruthy());
    expect(readyConnections.mock.calls.slice(callCount).some(([ids]) => ids.length === 0)).toBe(false);
    await waitFor(() => expect(activity).toHaveBeenCalledTimes(1));
  });

  it("opens independent split sessions and closes them on unmount", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const open = vi.spyOn(bridge, "openTerminal");
    const close = vi.spyOn(bridge, "closeTerminal");
    const user = userEvent.setup();
    const readyConnections = vi.fn();
    const view = render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={readyConnections} insertRequest={null} />);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("已连接")).toBeTruthy();
    await waitFor(() => expect(readyConnections).toHaveBeenCalledWith([connection.id]));
    await user.click(screen.getByRole("button", { name: "左右分屏" }));
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
    expect(xtermMock.terminalInstances).toHaveLength(2);
    view.unmount();
    await waitFor(() => expect(close).toHaveBeenCalledTimes(2));
  });

  it("refits an existing pane when the workspace becomes visible again", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const close = vi.spyOn(bridge, "closeTerminal");
    const onActivity = vi.fn();
    const onReadySessionsChange = vi.fn();
    const onReadyConnectionsChange = vi.fn();
    const view = render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={onActivity} onReadySessionsChange={onReadySessionsChange} onReadyConnectionsChange={onReadyConnectionsChange} insertRequest={null} />);
    await screen.findByText("已连接");
    const fit = xtermMock.fitInstances[0].fit;
    await waitFor(() => expect(fit).toHaveBeenCalled());
    const visibleFitCount = fit.mock.calls.length;

    view.rerender(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={null} profiles={[profile]} onActivity={onActivity} onReadySessionsChange={onReadySessionsChange} onReadyConnectionsChange={onReadyConnectionsChange} insertRequest={null} />);
    await waitFor(() => expect(view.container.querySelector(".connection-terminal")?.getAttribute("aria-hidden")).toBe("true"));
    view.rerender(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={onActivity} onReadySessionsChange={onReadySessionsChange} onReadyConnectionsChange={onReadyConnectionsChange} insertRequest={null} />);

    await waitFor(() => expect(fit.mock.calls.length).toBeGreaterThan(visibleFitCount));
    expect(close).not.toHaveBeenCalled();
  });

  it("passes a validated custom ANSI palette to xterm", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const custom = { ...profile, theme: "custom", palette: { ...DEFAULT_CUSTOM_PALETTE, background: "#123456", brightRed: "#fedcba" } };
    render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[custom]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);
    await screen.findByText("已连接");
    expect(xtermMock.terminalOptions[0]).toMatchObject({ theme: { background: "#123456", brightRed: "#fedcba" } });
  });

  it("applies the workspace default preset to connections using the default profile", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const defaultConnection = { ...connection, terminalProfileId: null };
    const darkProfile = { ...profile, theme: "firstshell-dark" };
    const lightProfile = { ...profile, revision: 2, theme: "firstshell-light" };
    const view = render(<TerminalWorkspace bridge={bridge} connections={[defaultConnection]} activeConnectionId={defaultConnection.id} profiles={[darkProfile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);
    await screen.findByText("已连接");
    expect(xtermMock.terminalOptions[0]).toMatchObject({ theme: { background: "#0e100e" } });

    view.rerender(<TerminalWorkspace bridge={bridge} connections={[defaultConnection]} activeConnectionId={defaultConnection.id} profiles={[lightProfile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);
    await waitFor(() => expect(xtermMock.terminalOptions).toHaveLength(2));
    expect(xtermMock.terminalOptions[1]).toMatchObject({ theme: { foreground: "#243128", background: "#f7f9f7" } });
  });

  it("requires explicit approval for pending host-key evidence", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const evidence: HostKeyEvidence = {
      host: "server.test",
      port: 22,
      algorithm: "ssh-ed25519",
      fingerprint: "SHA256:new",
      changed: false,
      previousFingerprint: null,
    };
    bridge.pendingHostKey = async () => evidence;
    bridge.openTerminal = async (_id, size, onEvent) => {
      const session = { id: "session-host-key", state: "connecting" as const, size };
      queueMicrotask(() => onEvent({ sessionId: session.id, event: { type: "error", code: "HOST_KEY_UNKNOWN", message: "unknown host key" } } as DesktopSessionEvent));
      return session;
    };
    const approve = vi.spyOn(bridge, "approveHostKey");
    const user = userEvent.setup();
    render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);
    expect(await screen.findByText("首次连接确认")).toBeTruthy();
    expect(screen.getByText("SHA256:new")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "信任并重连" }));
    expect(approve).toHaveBeenCalledWith("session-host-key", "SHA256:new", null);
  });

  it("keeps logging disabled until the user explicitly enables it", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const start = vi.spyOn(bridge, "startSessionLog");
    const stop = vi.spyOn(bridge, "stopSessionLog");
    const user = userEvent.setup();
    render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);
    await screen.findByText("已连接");
    expect(start).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "开启会话日志" }));
    await waitFor(() => expect(start).toHaveBeenCalledTimes(1));
    await user.click(screen.getByRole("button", { name: "停止会话日志" }));
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });

  it("uses the themed credential dialog and persists private-key passphrases", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const successfulOpen = bridge.openTerminal;
    bridge.openTerminal = vi.fn()
      .mockRejectedValueOnce({ code: "CREDENTIAL_REQUIRED", message: "SSH credential is not configured" })
      .mockImplementation(successfulOpen);
    const configureCredential = vi.spyOn(bridge, "configureCredential");
    const privateKeyConnection = {
      ...connection,
      authentication: "private_key" as const,
      privateKeyPath: "C:\\keys\\server.pem",
    };
    const user = userEvent.setup();
    render(<TerminalWorkspace bridge={bridge} connections={[privateKeyConnection]} activeConnectionId={privateKeyConnection.id} profiles={[profile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);

    await user.click(await screen.findByRole("button", { name: "配置凭据" }));
    expect(screen.getByRole("dialog", { name: "输入私钥口令" })).toBeTruthy();
    await user.type(screen.getByLabelText("私钥口令"), "passphrase-value");
    await user.click(screen.getByRole("button", { name: "保存并重连" }));
    await waitFor(() => expect(configureCredential).toHaveBeenCalledWith(privateKeyConnection.id, "passphrase-value"));
    expect(await screen.findByText("已连接")).toBeTruthy();
  });

  it("passes quick-connect passwords once without persisting them", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const successfulOpen = bridge.openQuickTerminal;
    bridge.openQuickTerminal = vi.fn()
      .mockRejectedValueOnce({ code: "CREDENTIAL_REQUIRED", message: "provide the temporary SSH credential before connecting" })
      .mockImplementation(successfulOpen);
    const configureCredential = vi.spyOn(bridge, "configureCredential");
    const temporaryConnection = {
      ...connection,
      id: "quick-test",
      authentication: "password" as const,
      temporary: true,
    };
    const user = userEvent.setup();
    render(<TerminalWorkspace bridge={bridge} connections={[temporaryConnection]} activeConnectionId={temporaryConnection.id} profiles={[profile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);

    await user.click(await screen.findByRole("button", { name: "配置凭据" }));
    expect(screen.getByRole("dialog", { name: "输入 SSH 密码" })).toBeTruthy();
    await user.type(screen.getByLabelText("密码"), "one-time-password");
    await user.click(screen.getByRole("button", { name: "连接" }));
    await waitFor(() => expect(bridge.openQuickTerminal).toHaveBeenLastCalledWith(expect.objectContaining({ credential: "one-time-password" }), expect.anything(), expect.any(Function)));
    expect(configureCredential).not.toHaveBeenCalled();
    expect(await screen.findByText("已连接")).toBeTruthy();
  });

  it("records completed commands and inserts snippet text without Enter", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const record = vi.spyOn(bridge, "recordCommand");
    const send = vi.spyOn(bridge, "sendTerminalInput");
    const view = render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} />);
    await waitFor(() => expect(xtermMock.terminalInstances[0]?.dataCallback).toBeTypeOf("function"));
    xtermMock.terminalInstances[0].dataCallback?.("whoami\r");
    await waitFor(() => expect(record).toHaveBeenCalledWith(connection.id, "whoami"));

    view.rerender(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={{ sequence: 1, connectionId: connection.id, text: "uptime" }} />);
    await waitFor(() => expect(send).toHaveBeenCalledWith(expect.any(String), [...new TextEncoder().encode("uptime")]));
    expect(send.mock.calls.some(([, data]) => data.at(-1) === 13 && new TextDecoder().decode(Uint8Array.from(data)).startsWith("uptime"))).toBe(false);
  });
});
