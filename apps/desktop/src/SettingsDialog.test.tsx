import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createPreviewBridge } from "./bridge";
import { SettingsDialog } from "./SettingsDialog";

afterEach(cleanup);

describe("SettingsDialog relay profiles", () => {
  it("submits the complete relay routing and trust configuration", async () => {
    const user = userEvent.setup();
    const bridge = createPreviewBridge();
    const snapshot = await bridge.snapshot();
    const onSaveProxy = vi.fn(async () => undefined);
    render(
      <SettingsDialog
        snapshot={snapshot}
        busy={false}
        error={null}
        onClose={() => undefined}
        onSaveProxy={onSaveProxy}
        onDeleteProxy={async () => undefined}
        onConfigureProxyCredential={async () => undefined}
        proxyCredentialStatus={async () => false}
        onSaveTerminal={async () => undefined}
        onDeleteTerminal={async () => undefined}
      />,
    );

    await user.click(screen.getByRole("button", { name: "新建代理配置" }));
    await user.type(screen.getByLabelText("名称"), "Production relay");
    await user.selectOptions(screen.getByLabelText("类型"), "relay");
    await user.type(screen.getByLabelText("中继主机"), "relay.example");
    await user.clear(screen.getByLabelText("QUIC/TLS 端口"));
    await user.type(screen.getByLabelText("QUIC/TLS 端口"), "9443");
    await user.clear(screen.getByLabelText("TCP/TLS 回退端口"));
    await user.type(screen.getByLabelText("TCP/TLS 回退端口"), "9444");
    await user.type(screen.getByLabelText("TLS 服务名"), "relay.example");
    await user.type(screen.getByLabelText("目标映射"), "ssh-prod");
    await user.type(screen.getByLabelText("中继证书路径（PEM / DER）"), "C:\\relay\\cert.pem");
    await user.click(screen.getByRole("button", { name: "保存" }));

    expect(onSaveProxy).toHaveBeenCalledWith(
      {
        kind: "relay",
        name: "Production relay",
        host: "relay.example",
        port: 9443,
        username: "",
        dnsMode: "remote",
        fallbackPort: 9444,
        relayServerName: "relay.example",
        relayMapping: "ssh-prod",
        relayCertificatePath: "C:\\relay\\cert.pem",
      },
      undefined,
    );
  });
});
