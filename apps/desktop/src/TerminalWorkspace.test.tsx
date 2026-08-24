import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { TerminalWorkspace } from "./TerminalWorkspace";
import type { ConnectionProfile, DesktopSessionEvent, HostKeyEvidence, TerminalProfile } from "./types";
import { DEFAULT_CUSTOM_PALETTE } from "./terminalThemes";

const xtermMock = vi.hoisted(() => {
  const terminalInstances: Array<{ dispose: ReturnType<typeof vi.fn>; dataCallback?: (data: string) => void }> = [];
  const terminalOptions: unknown[] = [];
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
  return { terminalInstances, terminalOptions, MockTerminal };
});

vi.mock("@xterm/xterm", () => ({ Terminal: xtermMock.MockTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit = vi.fn(); } }));

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
  xtermMock.terminalInstances.length = 0;
  xtermMock.terminalOptions.length = 0;
  vi.restoreAllMocks();
});

describe("M2 terminal workspace", () => {
  it("closes stale sessions created by StrictMode effect replay", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const open = vi.spyOn(bridge, "openTerminal");
    const close = vi.spyOn(bridge, "closeTerminal");
    const activity = vi.fn();
    render(<StrictMode><TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={activity} onReadySessionsChange={() => undefined} onReadyConnectionsChange={() => undefined} insertRequest={null} /></StrictMode>);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
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
