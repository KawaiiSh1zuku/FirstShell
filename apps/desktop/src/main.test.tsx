import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { App } from "./main";
import type { VaultStatus } from "./types";

vi.mock("./TerminalWorkspace", () => ({
  TerminalWorkspace: ({ insertRequest, onReadyConnectionsChange }: { insertRequest?: { sequence: number } | null; onReadyConnectionsChange(ids: string[]): void }) => <div data-testid="terminal-workspace"><span data-testid="insert-sequence">{insertRequest?.sequence ?? "none"}</span><button type="button" onClick={() => onReadyConnectionsChange(["connection-edge"])}>模拟终端就绪</button><button type="button" onClick={() => onReadyConnectionsChange([])}>模拟终端断开</button></div>,
}));

vi.mock("./ToolsWorkspace", async () => {
  const React = await import("react");
  return {
    ToolsWorkspace: ({ connections, onInsert }: { connections: Array<{ id: string }>; onInsert(connectionId: string, text: string): void }) => {
      const [localState, setLocalState] = React.useState(0);
      return <div data-testid="tools-workspace-mock"><span data-testid="tool-local-state">{localState}</span><button type="button" onClick={() => setLocalState((value) => value + 1)}>推进工具状态</button><button type="button" onClick={() => onInsert(connections[0].id, "echo repeated")}>测试插入</button></div>;
    },
  };
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.resolvedTheme;
});

describe("M1 workspace accessibility and vault state", () => {
  it("automatically unlocks an initialized vault with the OS device key", async () => {
    const bridge = createPreviewBridge();
    let status: VaultStatus = {
      initialized: true,
      unlocked: false,
      initializedThisRun: false,
      keyringAvailable: true,
      idleTimeoutSeconds: 900,
    };
    bridge.status = async () => status;
    const unlock = vi.fn(async () => {
      status = { ...status, unlocked: true };
      return bridge.snapshot();
    });
    bridge.unlock = unlock;

    render(<App bridge={bridge} />);
    expect(await screen.findByText("Edge Gateway")).toBeTruthy();
    expect(unlock).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("本地仓库已锁定")).toBeNull();
  });

  it("requires and confirms a recovery password before initialization", async () => {
    const bridge = createPreviewBridge();
    let status: VaultStatus = {
      initialized: false,
      unlocked: false,
      initializedThisRun: false,
      keyringAvailable: false,
      idleTimeoutSeconds: 900,
    };
    bridge.status = async () => status;
    bridge.initialize = async () => {
      status = { ...status, initialized: true, unlocked: true, initializedThisRun: true, keyringAvailable: true };
      return bridge.snapshot();
    };

    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    const password = await screen.findByLabelText("恢复口令");
    const confirmation = screen.getByLabelText("确认恢复口令");
    expect(document.activeElement).toBe(password);
    await user.type(password, "mandatory recovery password");
    await user.type(confirmation, "different password");
    expect((screen.getByRole("button", { name: "创建仓库" }) as HTMLButtonElement).disabled).toBe(true);
    await user.clear(confirmation);
    await user.type(confirmation, "mandatory recovery password");
    await user.click(screen.getByRole("button", { name: "创建仓库" }));
    expect(await screen.findByRole("tree", { name: "连接树" })).toBeTruthy();
  });

  it("switches to recovery password when the OS key is unavailable", async () => {
    const bridge = createPreviewBridge();
    let status: VaultStatus = {
      initialized: true,
      unlocked: false,
      initializedThisRun: false,
      keyringAvailable: false,
      idleTimeoutSeconds: 900,
    };
    bridge.status = async () => status;
    bridge.unlock = async () => {
      throw { code: "INVALID_STATE", message: "OS keyring key is unavailable; use recovery password", correlationId: "test" };
    };
    bridge.recover = async () => {
      status = { ...status, unlocked: true, keyringAvailable: true };
      return bridge.snapshot();
    };

    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    expect((await screen.findByRole("alert")).textContent).toContain("OS keyring key is unavailable");
    expect(screen.getByRole("button", { name: "恢复口令" }).getAttribute("aria-pressed")).toBe("true");
    await user.type(screen.getByLabelText("恢复口令"), "mandatory recovery password");
    await user.click(screen.getByRole("button", { name: "解锁" }));
    expect(await screen.findByText("Edge Gateway")).toBeTruthy();
  });

  it("supports keyboard panel resizing and removes decrypted DOM on lock", async () => {
    const bridge = createPreviewBridge();
    const saveLayout = vi.fn(bridge.saveLayout);
    bridge.saveLayout = saveLayout;
    let unlocked = true;
    bridge.status = async () => ({
      initialized: true,
      unlocked,
      initializedThisRun: false,
      keyringAvailable: true,
      idleTimeoutSeconds: 900,
    });
    bridge.lock = async () => { unlocked = false; };

    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    expect(await screen.findByText("Edge Gateway")).toBeTruthy();
    const separator = screen.getByRole("separator", { name: "调整连接面板宽度" });
    separator.focus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(saveLayout).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "锁定仓库" }));
    expect(await screen.findByText("本地仓库已锁定")).toBeTruthy();
    expect(screen.queryByText("Edge Gateway")).toBeNull();
  });

  it("collapses the connection panel and persists the layout preference", async () => {
    const bridge = createPreviewBridge();
    const saveLayout = vi.fn(bridge.saveLayout);
    bridge.saveLayout = saveLayout;
    const user = userEvent.setup();
    render(<App bridge={bridge} />);

    expect(await screen.findByRole("tree", { name: "连接树" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "收起连接栏" }));
    await waitFor(() => expect(screen.queryByRole("tree", { name: "连接树" })).toBeNull());
    expect(screen.queryByRole("separator", { name: "调整连接面板宽度" })).toBeNull();
    expect(screen.getByRole("separator", { name: "调整详情面板宽度" }).classList.contains("right-resize")).toBe(true);
    expect(saveLayout).toHaveBeenCalledWith(expect.objectContaining({ leftOpen: false }));

    await user.click(screen.getByRole("button", { name: "展开连接栏" }));
    expect(await screen.findByRole("tree", { name: "连接树" })).toBeTruthy();
    expect(saveLayout).toHaveBeenCalledWith(expect.objectContaining({ leftOpen: true }));
  });

  it("starts and stops host monitoring with a ready SSH terminal", async () => {
    const bridge = createPreviewBridge();
    const startHostMonitor = vi.fn(bridge.startHostMonitor);
    const stopHostMonitor = vi.fn(bridge.stopHostMonitor);
    bridge.startHostMonitor = startHostMonitor;
    bridge.stopHostMonitor = stopHostMonitor;
    const user = userEvent.setup();
    render(<App bridge={bridge} />);

    await user.click(await screen.findByRole("treeitem", { name: /^Edge Gateway/ }));
    const identity = screen.getByRole("heading", { name: "Edge Gateway" }).parentElement;
    expect(identity?.classList.contains("inspector-identity")).toBe(true);
    expect(identity?.textContent).toContain("SSH");
    expect(screen.getByText("终端连接后自动启动低开销监控")).toBeTruthy();
    await user.click(await screen.findByRole("button", { name: "模拟终端就绪" }));

    await waitFor(() => expect(startHostMonitor).toHaveBeenCalledWith("connection-edge", 5, expect.any(Function)));
    expect(await screen.findByText("监控中")).toBeTruthy();
    expect(screen.getByText("CPU")).toBeTruthy();
    expect(screen.getByText("内存")).toBeTruthy();
    expect(screen.getByText("SWAP")).toBeTruthy();
    expect(screen.getByText("1 分钟负载")).toBeTruthy();
    expect((screen.getByRole("combobox", { name: "监控网卡" }) as HTMLSelectElement).value).toBe("eth0");
    expect(screen.getByText("磁盘容量")).toBeTruthy();

    const onMonitorEvent = startHostMonitor.mock.calls[0][2];
    const sample = (capturedAtMs: number, eth0: [string, string], loopback: [string, string]) => ({
      version: 1,
      probe: { type: "host_metrics" as const },
      capturedAtMs,
      exitCode: 0,
      metrics: [{ key: "cpu_used", value: 20, unit: "percent" }],
      rows: [{ kind: "network", cells: ["eth0", ...eth0] }, { kind: "network", cells: ["lo", ...loopback] }],
      diagnostic: null,
    });
    act(() => {
      onMonitorEvent({ monitorId: "monitor-preview", state: "running", sample: sample(10_000, ["1000", "2000"], ["10", "20"]), diagnostic: null });
      onMonitorEvent({ monitorId: "monitor-preview", state: "running", sample: sample(15_000, ["6120", "7120"], ["1034", "2068"]), diagnostic: null });
    });
    expect(screen.getAllByText("1.0 KiB/s")).toHaveLength(2);
    await user.selectOptions(screen.getByRole("combobox", { name: "监控网卡" }), "lo");
    expect(screen.getByText("205 B/s")).toBeTruthy();
    expect(screen.getByText("410 B/s")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "模拟终端断开" }));
    await waitFor(() => expect(stopHostMonitor).toHaveBeenCalled());
  });

  it("restores a deleted tree item through the revision-checked undo action", async () => {
    const bridge = createPreviewBridge();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const user = userEvent.setup();
    render(<App bridge={bridge} />);

    await user.click(await screen.findByRole("treeitem", { name: /^Edge Gateway/ }));
    await user.click(screen.getByRole("button", { name: "删除所选项目" }));
    const undo = await screen.findByRole("button", { name: "撤销删除 Edge Gateway" });
    expect(screen.queryByText("Edge Gateway")).toBeNull();
    await user.click(undo);
    expect(await screen.findByRole("treeitem", { name: /^Edge Gateway/ })).toBeTruthy();
  });

  it("switches among light, dark, and system themes with persistence", async () => {
    const bridge = createPreviewBridge();
    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    await screen.findByText("Edge Gateway");

    const light = screen.getByRole("button", { name: "浅色" });
    const dark = screen.getByRole("button", { name: "深色" });
    const system = screen.getByRole("button", { name: "跟随系统" });
    await user.click(light);
    expect(light.getAttribute("aria-pressed")).toBe("true");
    expect(document.documentElement.dataset.resolvedTheme).toBe("light");
    expect(window.localStorage.getItem("firstshell.theme")).toBe("light");

    await user.click(dark);
    expect(dark.getAttribute("aria-pressed")).toBe("true");
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");

    await user.click(system);
    expect(system.getAttribute("aria-pressed")).toBe("true");
    expect(document.documentElement.dataset.theme).toBe("system");
    expect(window.localStorage.getItem("firstshell.theme")).toBe("system");
  });

  it("opens a temporary quick connection without adding it to the connection tree", async () => {
    const bridge = createPreviewBridge();
    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    await screen.findByText("Edge Gateway");
    await user.click(screen.getByRole("button", { name: "快速连接" }));
    await user.type(screen.getByLabelText("主机"), "temporary.example");
    await user.type(screen.getByLabelText("用户名"), "operator");
    await user.click(screen.getByRole("button", { name: "连接" }));
    expect(await screen.findByRole("tab", { name: /temporary\.example/ })).toBeTruthy();
    expect(screen.queryByRole("treeitem", { name: /temporary\.example/ })).toBeNull();
  });

  it("keeps the efficiency workspace mounted while showing connection details", async () => {
    const bridge = createPreviewBridge();
    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    await screen.findByText("Edge Gateway");

    await user.click(screen.getByRole("button", { name: "工具" }));
    await user.click(await screen.findByRole("button", { name: "推进工具状态" }));
    expect(screen.getByTestId("tool-local-state").textContent).toBe("1");

    await user.click(screen.getByRole("button", { name: "详情" }));
    expect(screen.getByTestId("tools-workspace-mock").closest("[hidden]")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "工具" }));
    expect(screen.getByTestId("tool-local-state").textContent).toBe("1");
  });

  it("uses a monotonic sequence for repeated insert requests in the same millisecond", async () => {
    vi.spyOn(Date, "now").mockReturnValue(1_000);
    const bridge = createPreviewBridge();
    const user = userEvent.setup();
    render(<App bridge={bridge} />);
    await screen.findByText("Edge Gateway");

    await user.click(screen.getByRole("button", { name: "工具" }));
    const insert = await screen.findByRole("button", { name: "测试插入" });
    await user.click(insert);
    expect((await screen.findByTestId("insert-sequence")).textContent).toBe("1");
    await user.click(insert);
    expect(screen.getByTestId("insert-sequence").textContent).toBe("2");
  });
});
