import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { OperationsWorkspace } from "./OperationsWorkspace";
import type { ConnectionProfile, TransferSnapshot } from "./types";

const connection: ConnectionProfile = {
  id: "connection-operations",
  folderId: null,
  revision: 1,
  sortOrder: 0,
  kind: "ssh",
  name: "Operations SSH",
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

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("M3 operations workspace", () => {
  it("automatically pairs a ready terminal with a rooted SFTP tree and address bar", async () => {
    const bridge = createPreviewBridge();
    const open = vi.spyOn(bridge, "openSftp");
    const close = vi.spyOn(bridge, "closeSftp");
    const list = vi.spyOn(bridge, "listSftp");
    const user = userEvent.setup();
    const props = { bridge, connections: [connection], activeConnectionId: connection.id, activities: [], onActivity: () => undefined, onCollapse: () => undefined };
    const view = render(<OperationsWorkspace {...props} readyConnectionIds={[connection.id]} />);

    expect(await screen.findByRole("tree", { name: "远程目录树" })).toBeTruthy();
    expect(open).toHaveBeenCalledWith(connection.id, ".");
    const address = screen.getByRole("textbox", { name: "远程地址" });
    await waitFor(() => expect((address as HTMLInputElement).value).toBe("/home/preview"));
    await user.clear(address);
    await user.type(address, "/home/preview/logs{Enter}");
    await waitFor(() => expect(list).toHaveBeenCalledWith(expect.any(String), "logs"));
    await user.clear(address);
    await user.type(address, "/etc{Enter}");
    expect(address.getAttribute("aria-invalid")).toBe("true");
    expect(list).not.toHaveBeenCalledWith(expect.any(String), "etc");

    view.rerender(<OperationsWorkspace {...props} readyConnectionIds={[]} />);
    await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
  });

  it("opens a rooted SFTP browser and starts an explicit tunnel", async () => {
    const bridge = createPreviewBridge();
    const user = userEvent.setup();
    render(<OperationsWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} readyConnectionIds={[]} activities={[]} onActivity={() => undefined} onCollapse={() => undefined} />);

    await user.click(screen.getByRole("button", { name: "连接 SFTP" }));
    expect(await screen.findByText(/README\.txt/)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: /^隧道/ }));
    await user.click(screen.getByRole("button", { name: "启动" }));
    expect(await screen.findByText("127.0.0.1:49152")).toBeTruthy();
  });

  it("moves native multi-file selections into the bounded transfer view", async () => {
    const bridge = createPreviewBridge();
    const queued: TransferSnapshot = {
      id: "transfer-test",
      workspaceId: "sftp-test",
      generation: 0,
      direction: "upload",
      name: "archive.bin",
      remotePath: "archive.bin",
      status: "queued",
      transferred: 0,
      total: 1024,
      resumed: false,
      sha256: null,
      error: null,
    };
    bridge.chooseSftpUploads = async (_workspaceId, _directory, _options, onEvent) => {
      queueMicrotask(() => onEvent({ ...queued, status: "completed", transferred: 1024 }));
      return [queued];
    };
    const user = userEvent.setup();
    render(<OperationsWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} readyConnectionIds={[]} activities={[]} onActivity={() => undefined} onCollapse={() => undefined} />);

    await user.click(screen.getByRole("button", { name: "连接 SFTP" }));
    await screen.findByText(/README\.txt/);
    await user.click(screen.getByRole("button", { name: "多文件上传" }));
    expect(await screen.findByText("archive.bin")).toBeTruthy();
    await waitFor(() => expect(screen.getByText(/已完成/)).toBeTruthy());
  });

  it("consumes an opaque Rust-staged drop without receiving local paths", async () => {
    const bridge = createPreviewBridge();
    let notifyDrop: Parameters<typeof bridge.onSftpUploadDrop>[0] | undefined;
    bridge.onSftpUploadDrop = async (callback) => {
      notifyDrop = callback;
      return () => undefined;
    };
    const queued: TransferSnapshot = {
      id: "transfer-drop",
      workspaceId: "sftp-drop",
      generation: 0,
      direction: "upload",
      name: "dropped.bin",
      remotePath: "dropped.bin",
      status: "queued",
      transferred: 0,
      total: 2048,
      resumed: false,
      sha256: null,
      error: null,
    };
    bridge.queueSftpDrop = vi.fn(async () => [queued]);
    const user = userEvent.setup();
    render(<OperationsWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} readyConnectionIds={[]} activities={[]} onActivity={() => undefined} onCollapse={() => undefined} />);

    await user.click(screen.getByRole("button", { name: "连接 SFTP" }));
    await screen.findByText(/README\.txt/);
    await waitFor(() => expect(notifyDrop).toBeTypeOf("function"));
    act(() => notifyDrop?.({
      batchId: "opaque-drop-id",
      files: [{ name: "dropped.bin", size: 2048 }],
      error: null,
    }));

    expect(await screen.findByText("dropped.bin")).toBeTruthy();
    expect(bridge.queueSftpDrop).toHaveBeenCalledWith(
      expect.any(String),
      "opaque-drop-id",
      "",
      { conflict: "resume", verify: "size" },
      expect.any(Function),
    );
  });

  it("pauses and resumes a transfer without conflating pause with cancel", async () => {
    const bridge = createPreviewBridge();
    let transfer: TransferSnapshot = {
      id: "transfer-pause",
      workspaceId: "sftp-pause",
      generation: 0,
      direction: "upload",
      name: "large.bin",
      remotePath: "large.bin",
      status: "running",
      transferred: 1024,
      total: 4096,
      resumed: false,
      sha256: null,
      error: null,
    };
    bridge.listTransfers = async () => [transfer];
    bridge.pauseTransfer = vi.fn(async () => { transfer = { ...transfer, status: "paused" }; });
    bridge.retryTransfer = vi.fn(async (_id, onEvent) => {
      const queued = { ...transfer, generation: 1, status: "queued" as const, transferred: 0 };
      transfer = { ...queued, status: "completed", transferred: 4096 };
      onEvent(transfer);
      return queued;
    });
    const user = userEvent.setup();
    render(<OperationsWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} readyConnectionIds={[]} activities={[]} onActivity={() => undefined} onCollapse={() => undefined} />);

    await user.click(screen.getByRole("button", { name: /^传输/ }));
    await user.click(await screen.findByRole("button", { name: "暂停 large.bin" }));
    expect(await screen.findByText(/已暂停/)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "恢复 large.bin" }));
    expect(bridge.pauseTransfer).toHaveBeenCalledWith("transfer-pause");
    expect(bridge.retryTransfer).toHaveBeenCalledWith("transfer-pause", expect.any(Function));
    expect(await screen.findByText(/已完成/)).toBeTruthy();
  });

  it("edits with a conflict fingerprint and exposes controlled archive and ZMODEM actions", async () => {
    const bridge = createPreviewBridge();
    const openText = vi.spyOn(bridge, "openRemoteText");
    const saveText = vi.spyOn(bridge, "saveRemoteText");
    const archive = vi.spyOn(bridge, "createRemoteArchive");
    const zmodemUpload = vi.spyOn(bridge, "chooseZmodemUpload");
    vi.spyOn(window, "prompt").mockReturnValue("README.tar.gz");
    const user = userEvent.setup();
    render(<OperationsWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} readyConnectionIds={[]} activities={[]} onActivity={() => undefined} onCollapse={() => undefined} />);

    await user.click(screen.getByRole("button", { name: "连接 SFTP" }));
    const readme = await screen.findByRole("row", { name: /README\.txt/ });
    await user.click(readme);
    await user.click(screen.getByRole("button", { name: "编辑远程文本" }));
    const editor = await screen.findByRole("textbox", { name: "远程文件内容" });
    await user.clear(editor);
    await user.type(editor, "updated remotely");
    await user.click(screen.getByRole("button", { name: "安全保存" }));
    expect(openText).toHaveBeenCalledWith(expect.any(String), "README.txt");
    expect(saveText).toHaveBeenCalledWith(expect.any(String), "README.txt", "preview-fingerprint", "updated remotely", false);
    await user.click(screen.getByRole("button", { name: "关闭远程编辑器" }));

    await user.click(await screen.findByRole("row", { name: /README\.txt/ }));
    await user.click(screen.getByRole("button", { name: "创建远程压缩包" }));
    expect(archive).toHaveBeenCalledWith(expect.any(String), ["README.txt"], "README.tar.gz");
    await user.click(screen.getByRole("button", { name: "ZMODEM 上传" }));
    expect(zmodemUpload).toHaveBeenCalledWith(expect.any(String), "", expect.any(Function));
  });
});
