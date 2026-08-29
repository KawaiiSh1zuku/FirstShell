import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { RdpWorkspace } from "./RdpWorkspace";
import type { ConnectionProfile } from "./types";

const connection: ConnectionProfile = {
  id: "rdp-1",
  folderId: null,
  revision: 1,
  sortOrder: 0,
  kind: "rdp",
  name: "Windows Server",
  host: "rdp.example",
  port: 3389,
  username: "operator",
  proxyId: null,
  jumpHostIds: [],
  terminalProfileId: null,
  authentication: "password",
  privateKeyPath: "",
  allowLegacyAlgorithms: false,
  opensshFallback: false,
  tags: [],
  notes: "",
};

afterEach(cleanup);

describe("RdpWorkspace", () => {
  it("opens the isolated helper and renders its companion state", async () => {
    const bridge = createPreviewBridge();
    const openRdp = vi.spyOn(bridge, "openRdp");
    render(<RdpWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} onActivity={() => undefined} />);

    await waitFor(() => expect(openRdp).toHaveBeenCalledWith(
      connection.id,
      expect.objectContaining({ width: 640, height: 480 }),
      expect.any(Function),
    ));
    expect(await screen.findByText("远程桌面已在伴随窗口中运行")).toBeTruthy();
  });

  it("updates a credential through the native bridge before reconnecting", async () => {
    const user = userEvent.setup();
    const bridge = createPreviewBridge();
    const configure = vi.spyOn(bridge, "configureRdpCredential");
    render(<RdpWorkspace bridge={bridge} connections={[connection]} activeConnectionId={connection.id} onActivity={() => undefined} />);

    await user.click(await screen.findByRole("button", { name: "更新 RDP 凭据" }));
    await user.type(screen.getByLabelText("RDP 密码"), "one-time-secret");
    await user.click(screen.getByRole("button", { name: "保存并重连" }));

    await waitFor(() => expect(configure).toHaveBeenCalledWith(connection.id, "one-time-secret"));
  });
});
