import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { TerminalWorkspace } from "./TerminalWorkspace";
import type { ConnectionProfile, DesktopSessionEvent, HostKeyEvidence, TerminalProfile } from "./types";

const xtermMock = vi.hoisted(() => {
  const terminalInstances: Array<{ dispose: ReturnType<typeof vi.fn> }> = [];
  class MockTerminal {
    cols = 100;
    rows = 30;
    write = vi.fn();
    writeln = vi.fn();
    focus = vi.fn();
    dispose = vi.fn();
    loadAddon = vi.fn();
    open = vi.fn();
    onData = vi.fn(() => ({ dispose: vi.fn() }));
    constructor() { terminalInstances.push(this); }
  }
  return { terminalInstances, MockTerminal };
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
  scrollback: 10000,
  encoding: "utf8",
  keymap: "xterm",
};

afterEach(() => {
  cleanup();
  xtermMock.terminalInstances.length = 0;
  vi.restoreAllMocks();
});

describe("M2 terminal workspace", () => {
  it("closes stale sessions created by StrictMode effect replay", async () => {
    vi.stubGlobal("ResizeObserver", MockResizeObserver);
    const bridge = createPreviewBridge();
    const open = vi.spyOn(bridge, "openTerminal");
    const close = vi.spyOn(bridge, "closeTerminal");
    const activity = vi.fn();
    render(<StrictMode><TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={activity} onReadyCountChange={() => undefined} /></StrictMode>);
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
    const view = render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadyCountChange={() => undefined} />);
    await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("已连接")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "左右分屏" }));
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2));
    expect(xtermMock.terminalInstances).toHaveLength(2);
    view.unmount();
    await waitFor(() => expect(close).toHaveBeenCalledTimes(2));
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
    render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadyCountChange={() => undefined} />);
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
    render(<TerminalWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} profiles={[profile]} onActivity={() => undefined} onReadyCountChange={() => undefined} />);
    await screen.findByText("已连接");
    expect(start).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "开启会话日志" }));
    await waitFor(() => expect(start).toHaveBeenCalledTimes(1));
    await user.click(screen.getByRole("button", { name: "停止会话日志" }));
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  });
});
