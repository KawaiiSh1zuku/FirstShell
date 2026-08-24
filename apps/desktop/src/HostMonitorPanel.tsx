import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { WorkspaceBridge } from "./bridge";
import type { MonitorEvent, MonitorSnapshot, ProbeMetric, ProbeResult } from "./types";

const MONITOR_INTERVAL_SECONDS = 5;
const MONITOR_HISTORY_LIMIT = 60;

export interface AutoMonitorView {
  monitor: MonitorSnapshot | null;
  history: ProbeResult[];
  error: string | null;
  starting: boolean;
}

interface MonitorHandle {
  active: boolean;
  monitorId: string | null;
}

export function useAutomaticHostMonitors(
  bridge: WorkspaceBridge,
  connectionIds: string[],
  onActivity: (message: string) => void,
) {
  const [views, setViews] = useState<Record<string, AutoMonitorView>>({});
  const handlesRef = useRef(new Map<string, MonitorHandle>());
  const connectionKey = useMemo(() => [...new Set(connectionIds)].sort().join("\u0000"), [connectionIds]);

  const updateView = useCallback((connectionId: string, update: (current: AutoMonitorView) => AutoMonitorView) => {
    setViews((current) => ({
      ...current,
      [connectionId]: update(current[connectionId] ?? { monitor: null, history: [], error: null, starting: true }),
    }));
  }, []);

  useEffect(() => {
    const wanted = new Set(connectionKey ? connectionKey.split("\u0000") : []);
    for (const [connectionId, handle] of handlesRef.current) {
      if (wanted.has(connectionId)) continue;
      handle.active = false;
      handlesRef.current.delete(connectionId);
      if (handle.monitorId) void bridge.stopHostMonitor(handle.monitorId).catch(() => undefined);
    }
    setViews((current) => Object.fromEntries(Object.entries(current).filter(([connectionId]) => wanted.has(connectionId))));

    for (const connectionId of wanted) {
      if (handlesRef.current.has(connectionId)) continue;
      const handle: MonitorHandle = { active: true, monitorId: null };
      handlesRef.current.set(connectionId, handle);
      updateView(connectionId, () => ({ monitor: null, history: [], error: null, starting: true }));
      const onEvent = (event: MonitorEvent) => {
        if (!handle.active) return;
        updateView(connectionId, (current) => ({
          ...current,
          monitor: current.monitor ? { ...current.monitor, state: event.state, sampleCount: current.monitor.sampleCount + (event.sample ? 1 : 0), diagnostic: event.diagnostic } : null,
          history: event.sample ? [...current.history, event.sample].slice(-MONITOR_HISTORY_LIMIT) : current.history,
          error: event.diagnostic,
          starting: false,
        }));
      };
      void bridge.startHostMonitor(connectionId, MONITOR_INTERVAL_SECONDS, onEvent).then((monitor) => {
        if (!handle.active) {
          void bridge.stopHostMonitor(monitor.id).catch(() => undefined);
          return;
        }
        handle.monitorId = monitor.id;
        updateView(connectionId, (current) => ({ ...current, monitor, error: monitor.diagnostic, starting: false }));
        onActivity(monitor.state === "host_key_required" ? "自动监控等待主机密钥确认" : "主机监控已自动启动");
      }).catch((reason) => {
        if (!handle.active) return;
        updateView(connectionId, (current) => ({ ...current, error: message(reason), starting: false }));
      });
    }
  }, [bridge, connectionKey, onActivity, updateView]);

  useEffect(() => () => {
    for (const handle of handlesRef.current.values()) {
      handle.active = false;
      if (handle.monitorId) void bridge.stopHostMonitor(handle.monitorId).catch(() => undefined);
    }
    handlesRef.current.clear();
  }, [bridge]);

  const approve = useCallback(async (connectionId: string) => {
    const current = views[connectionId];
    if (!current?.monitor?.hostKey) return;
    updateView(connectionId, (value) => ({ ...value, error: null, starting: true }));
    try {
      const monitor = await bridge.approveMonitorHostKey(
        current.monitor.id,
        current.monitor.hostKey.fingerprint,
        current.monitor.hostKey.previousFingerprint,
      );
      updateView(connectionId, (value) => ({ ...value, monitor, starting: false }));
      onActivity("自动监控主机密钥已确认");
    } catch (reason) {
      updateView(connectionId, (value) => ({ ...value, error: message(reason), starting: false }));
    }
  }, [bridge, onActivity, updateView, views]);

  return { views, approve };
}

export function HostMonitorPanel({ connected, view, onApprove }: { connected: boolean; view?: AutoMonitorView; onApprove(): void }) {
  const latest = view?.history.at(-1);
  const previous = view?.history.at(-2);
  const networkRows = latest?.rows.filter((row) => row.kind === "network" && row.cells.length >= 3) ?? [];
  const interfaces = networkRows.map((row) => row.cells[0]);
  const [requestedInterface, setRequestedInterface] = useState("");
  const networkInterface = interfaces.includes(requestedInterface) ? requestedInterface : interfaces.find((value) => value !== "lo") ?? interfaces[0] ?? "";
  const network = networkRows.find((row) => row.cells[0] === networkInterface);
  const previousNetwork = previous?.rows.find((row) => row.kind === "network" && row.cells[0] === networkInterface);
  const elapsedSeconds = latest && previous ? Math.max(0, latest.capturedAtMs - previous.capturedAtMs) / 1000 : 0;
  const receiveRate = counterRate(network?.cells[1], previousNetwork?.cells[1], elapsedSeconds);
  const transmitRate = counterRate(network?.cells[2], previousNetwork?.cells[2], elapsedSeconds);
  const cpuUsed = metric(latest, "cpu_used") ?? cpuUsage(latest, previous);
  const memoryTotal = metric(latest, "memory_total");
  const memoryAvailable = metric(latest, "memory_available");
  const memoryUsed = memoryTotal !== undefined && memoryAvailable !== undefined ? Math.max(0, memoryTotal - memoryAvailable) : undefined;
  const swapTotal = metric(latest, "swap_total");
  const swapFree = metric(latest, "swap_free");
  const swapUsed = swapTotal !== undefined && swapFree !== undefined ? Math.max(0, swapTotal - swapFree) : undefined;
  const load = metric(latest, "load_1m");
  const disks = latest?.rows.filter((row) => row.kind === "disk" && row.cells.length >= 3) ?? [];

  return <section className="host-monitor-summary" aria-label="主机监控">
    <div className="host-monitor-heading"><strong>主机状态</strong><span className={view?.monitor?.state ?? "idle"}>{monitorStatus(connected, view)}</span></div>
    {!connected && <div className="monitor-placeholder">终端连接后自动启动低开销监控</div>}
    {connected && view?.monitor?.state === "host_key_required" && view.monitor.hostKey && <div className="tool-host-key"><strong>监控连接需要确认主机密钥</strong><code>{view.monitor.hostKey.fingerprint}</code><button type="button" onClick={onApprove}>信任并启动</button></div>}
    {view?.error && <div className="tool-error">{view.error}</div>}
    {latest && <>
      <div className="host-metric-grid">
        <MetricCard label="CPU" value={formatPercent(cpuUsed)} />
        <MetricCard label="内存" value={formatUsage(memoryUsed, memoryTotal)} />
        <MetricCard label="SWAP" value={swapTotal === 0 ? "未启用" : formatUsage(swapUsed, swapTotal)} />
        <MetricCard label="1 分钟负载" value={load === undefined ? "—" : load.toFixed(2)} />
      </div>
      <div className="host-monitor-section">
        <div className="host-monitor-section-title"><strong>网速</strong>{interfaces.length > 0 && <select aria-label="监控网卡" value={networkInterface} onChange={(event) => setRequestedInterface(event.target.value)}>{interfaces.map((value) => <option key={value}>{value}</option>)}</select>}</div>
        <div className="network-rates"><span>↓ <strong>{formatRate(receiveRate)}</strong></span><span>↑ <strong>{formatRate(transmitRate)}</strong></span></div>
      </div>
      <div className="host-monitor-section"><div className="host-monitor-section-title"><strong>磁盘容量</strong></div><div className="disk-list">{disks.map((row) => <DiskRow key={row.cells[0]} cells={row.cells} />)}{disks.length === 0 && <span className="monitor-placeholder">暂无磁盘数据</span>}</div></div>
    </>}
  </section>;
}

function MetricCard({ label, value }: { label: string; value: string }) {
  return <div><small>{label}</small><strong>{value}</strong></div>;
}

function DiskRow({ cells }: { cells: string[] }) {
  const total = finiteNumber(cells[1]) ?? 0;
  const available = finiteNumber(cells[2]) ?? 0;
  const used = Math.max(0, total - available);
  return <div className="disk-row"><div><strong>{cells[0]}</strong><span>{formatBytes(used)} / {formatBytes(total)}</span></div><progress max={Math.max(total, 1)} value={used} /></div>;
}

function metric(sample: ProbeResult | undefined, key: string) {
  return sample?.metrics.find((value: ProbeMetric) => value.key === key)?.value;
}

function cpuUsage(latest: ProbeResult | undefined, previous: ProbeResult | undefined) {
  const total = metric(latest, "cpu_total");
  const idle = metric(latest, "cpu_idle");
  const previousTotal = metric(previous, "cpu_total");
  const previousIdle = metric(previous, "cpu_idle");
  if ([total, idle, previousTotal, previousIdle].some((value) => value === undefined)) return undefined;
  const totalDelta = total! - previousTotal!;
  if (totalDelta <= 0) return undefined;
  return Math.max(0, Math.min(100, (1 - (idle! - previousIdle!) / totalDelta) * 100));
}

function counterRate(current: string | undefined, previous: string | undefined, elapsedSeconds: number) {
  const currentValue = finiteNumber(current);
  const previousValue = finiteNumber(previous);
  if (currentValue === undefined || previousValue === undefined || elapsedSeconds <= 0 || currentValue < previousValue) return undefined;
  return (currentValue - previousValue) / elapsedSeconds;
}

function finiteNumber(value: string | undefined) {
  if (value === undefined) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function formatPercent(value: number | undefined) {
  return value === undefined ? "等待采样" : `${value.toFixed(1)}%`;
}

function formatUsage(used: number | undefined, total: number | undefined) {
  if (used === undefined || total === undefined || total <= 0) return "—";
  return `${(used / total * 100).toFixed(1)}% · ${formatBytes(used)}`;
}

function formatRate(value: number | undefined) {
  return value === undefined ? "等待采样" : `${formatBytes(value)}/s`;
}

function formatBytes(value: number) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let scaled = Math.max(0, value);
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  return `${scaled >= 100 || index === 0 ? scaled.toFixed(0) : scaled.toFixed(1)} ${units[index]}`;
}

function monitorStatus(connected: boolean, view?: AutoMonitorView) {
  if (!connected) return "未连接";
  if (view?.starting || !view) return "启动中";
  if (view.monitor?.state === "host_key_required") return "待确认";
  if (view.monitor?.state === "degraded") return "已降级";
  if (view.monitor?.state === "running") return "监控中";
  return view.error ? "不可用" : "等待中";
}

function message(reason: unknown) {
  return reason instanceof Error ? reason.message : typeof reason === "object" && reason && "message" in reason ? String(reason.message) : "自动监控失败";
}
