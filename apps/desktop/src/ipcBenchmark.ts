import { Channel, invoke } from "@tauri-apps/api/core";

export interface IpcBenchmarkEvent {
  sequence: number;
  kind: "terminal" | "monitor";
  payload: string;
}

export interface IpcBenchmarkResult {
  sent: number;
  received: number;
  backendElapsedMicros: number;
  webviewElapsedMillis: number;
}

interface BackendSummary {
  sent: number;
  backendElapsedMicros: number;
}

export async function measureIpcChannel(
  totalEvents = 10_000,
  payloadBytes = 1024,
): Promise<IpcBenchmarkResult> {
  const channel = new Channel<IpcBenchmarkEvent>();
  let received = 0;
  let resolveAll: (() => void) | undefined;
  const allReceived = new Promise<void>((resolve) => {
    resolveAll = resolve;
  });
  channel.onmessage = () => {
    received += 1;
    if (received === totalEvents) resolveAll?.();
  };

  const started = performance.now();
  const summary = await invoke<BackendSummary>("run_ipc_benchmark", {
    totalEvents,
    payloadBytes,
    onEvent: channel,
  });
  await Promise.race([
    allReceived,
    new Promise<never>((_, reject) => {
      window.setTimeout(() => reject(new Error("IPC benchmark timed out")), 10_000);
    }),
  ]);

  return {
    sent: summary.sent,
    received,
    backendElapsedMicros: summary.backendElapsedMicros,
    webviewElapsedMillis: performance.now() - started,
  };
}
