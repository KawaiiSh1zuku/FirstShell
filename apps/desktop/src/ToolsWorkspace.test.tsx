import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { ToolsWorkspace } from "./ToolsWorkspace";
import type { ConnectionProfile, Snippet } from "./types";

const connection: ConnectionProfile = {
  id: "connection-edge",
  folderId: null,
  revision: 1,
  sortOrder: 0,
  kind: "ssh",
  name: "Edge Gateway",
  host: "server.test",
  port: 22,
  username: "operator",
  proxyId: null,
  jumpHostIds: [],
  terminalProfileId: null,
  authentication: "agent",
  privateKeyPath: "",
  allowLegacyAlgorithms: false,
  opensshFallback: false,
  tags: [],
  notes: "",
};

const snippet: Snippet = {
  id: "snippet-preview",
  revision: 1,
  name: "查看服务日志",
  tags: ["systemd"],
  template: "journalctl -u {{service}} -n {{lines}}",
  parameters: [
    { name: "service", label: "服务", kind: "text", required: true, defaultValue: "nginx", choices: [] },
    { name: "lines", label: "行数", kind: "integer", required: true, defaultValue: "100", choices: [] },
  ],
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderTools(overrides: Partial<React.ComponentProps<typeof ToolsWorkspace>> = {}) {
  const bridge = createPreviewBridge();
  const onInsert = vi.fn();
  const props = {
    bridge,
    connections: [connection],
    snippets: [snippet],
    activeConnectionId: connection.id,
    readySessions: [{ sessionId: "session-ready", connectionId: connection.id }],
    onInsert,
    onRefresh: async () => undefined,
    onActivity: () => undefined,
    ...overrides,
  };
  render(<ToolsWorkspace {...props} />);
  return { bridge, onInsert };
}

describe("M4 efficiency tools", () => {
  it("renders typed snippets and inserts text without executing it", async () => {
    const user = userEvent.setup();
    const { onInsert } = renderTools();
    await user.click(screen.getByRole("button", { name: "片段" }));
    await user.clear(screen.getByLabelText("服务"));
    await user.type(screen.getByLabelText("服务"), "sshd");
    await user.clear(screen.getByLabelText("行数"));
    await user.type(screen.getByLabelText("行数"), "25");
    await user.click(screen.getByRole("button", { name: "插入活动终端" }));
    expect(onInsert).toHaveBeenCalledWith(connection.id, "journalctl -u sshd -n 25");
  });

  it("requires explicit targets and confirmation before atomic broadcast", async () => {
    const user = userEvent.setup();
    const { bridge } = renderTools();
    const broadcast = vi.spyOn(bridge, "broadcastTerminal");
    const stop = vi.spyOn(bridge, "stopBroadcast");
    await user.click(screen.getByRole("button", { name: "广播" }));
    const execute = screen.getByRole("button", { name: "广播执行" });
    expect((execute as HTMLButtonElement).disabled).toBe(true);
    await user.click(screen.getByRole("checkbox", { name: /Edge Gateway/ }));
    await user.type(screen.getByRole("textbox", { name: "广播命令" }), "uptime");
    await user.click(screen.getByRole("checkbox", { name: /我确认/ }));
    await user.click(execute);
    expect(broadcast).toHaveBeenCalledWith(["session-ready"], "uptime", true);
    await user.click(screen.getByRole("button", { name: "紧急停止" }));
    expect(stop).toHaveBeenCalledWith(["session-ready"]);
  });

  it("starts bounded monitoring and reuses encrypted command history for insertion", async () => {
    const user = userEvent.setup();
    const bridge = createPreviewBridge();
    await bridge.recordCommand(connection.id, "cd /var/log");
    const onInsert = vi.fn();
    renderTools({ bridge, onInsert });
    await user.click(screen.getByRole("button", { name: "启动" }));
    expect(await screen.findByText("load_1m")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "历史" }));
    await user.click(await screen.findByText("cd /var/log"));
    await waitFor(() => expect(onInsert).toHaveBeenCalledWith(connection.id, "cd /var/log"));
  });
});
