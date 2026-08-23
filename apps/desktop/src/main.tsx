import { StrictMode, useCallback, useEffect, useMemo, useReducer } from "react";
import { createRoot } from "react-dom/client";
import {
  Activity,
  Archive,
  ChevronDown,
  ChevronRight,
  Circle,
  Edit3,
  Folder as FolderIcon,
  FolderPlus,
  HardDrive,
  Laptop,
  Lock,
  Monitor,
  MoreHorizontal,
  Plus,
  Search,
  Server,
  Settings,
  TerminalSquare,
  Trash2,
  Undo2,
  X,
} from "lucide-react";
import { BackupDialog } from "./BackupDialog";
import { workspaceBridge, type WorkspaceBridge } from "./bridge";
import { EntityDialog } from "./EntityDialog";
import { SettingsDialog } from "./SettingsDialog";
import { ThemeControl, useThemeMode } from "./ThemeControl";
import type {
  ConnectionInput,
  ConnectionProfile,
  Folder,
  FolderInput,
  ProxyInput,
  ProxyProfile,
  PublicError,
  SearchResult,
  Selection,
  TerminalProfile,
  TerminalProfileInput,
  VaultStatus,
  WorkspaceLayout,
  WorkspaceSnapshot,
} from "./types";
import { VaultGate } from "./VaultGate";
import "./styles.css";

type EntityDialogState =
  | { kind: "folder"; value?: Folder }
  | { kind: "connection"; value?: ConnectionProfile }
  | null;

interface ActivityItem {
  id: number;
  message: string;
  time: string;
}

interface UndoDelete {
  kind: "folder" | "connection";
  id: string;
  expectedRevision: number;
  label: string;
}

interface AppState {
  status: VaultStatus | null;
  snapshot: WorkspaceSnapshot | null;
  layout: WorkspaceLayout;
  selection: Selection;
  openTabs: string[];
  activeTab: string | null;
  query: string;
  searchResults: SearchResult[];
  busy: boolean;
  error: string | null;
  entityDialog: EntityDialogState;
  settingsOpen: boolean;
  backupOpen: boolean;
  activities: ActivityItem[];
  undoDelete: UndoDelete | null;
}

type Action =
  | { type: "patch"; value: Partial<AppState> }
  | { type: "locked"; status: VaultStatus }
  | { type: "activity"; message: string };

const defaultLayout: WorkspaceLayout = {
  leftWidth: 256,
  rightWidth: 288,
  bottomHeight: 180,
  bottomOpen: true,
};

const initialState: AppState = {
  status: null,
  snapshot: null,
  layout: defaultLayout,
  selection: null,
  openTabs: [],
  activeTab: null,
  query: "",
  searchResults: [],
  busy: false,
  error: null,
  entityDialog: null,
  settingsOpen: false,
  backupOpen: false,
  activities: [],
  undoDelete: null,
};

function reducer(state: AppState, action: Action): AppState {
  if (action.type === "patch") return { ...state, ...action.value };
  if (action.type === "locked") {
    return {
      ...state,
      status: action.status,
      snapshot: null,
      selection: null,
      openTabs: [],
      activeTab: null,
      query: "",
      searchResults: [],
      entityDialog: null,
      settingsOpen: false,
      backupOpen: false,
      error: null,
      undoDelete: null,
    };
  }
  const item = {
    id: Date.now() + state.activities.length,
    message: action.message,
    time: new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
  };
  return { ...state, activities: [item, ...state.activities].slice(0, 50) };
}

function errorMessage(error: unknown): string {
  if (typeof error === "object" && error && "message" in error) {
    return String((error as PublicError).message);
  }
  return error instanceof Error ? error.message : "操作失败";
}

export function App({ bridge = workspaceBridge }: { bridge?: WorkspaceBridge }) {
  const [state, dispatch] = useReducer(reducer, initialState);
  const [themeMode, setThemeMode] = useThemeMode();

  const perform = useCallback(async <T,>(operation: () => Promise<T>, activity?: string): Promise<T> => {
    dispatch({ type: "patch", value: { busy: true, error: null } });
    try {
      const result = await operation();
      if (activity) dispatch({ type: "activity", message: activity });
      return result;
    } catch (error) {
      dispatch({ type: "patch", value: { error: errorMessage(error) } });
      throw error;
    } finally {
      dispatch({ type: "patch", value: { busy: false } });
    }
  }, []);

  const loadWorkspace = useCallback(async (snapshot?: WorkspaceSnapshot) => {
    const [nextSnapshot, layout, status] = await Promise.all([
      snapshot ? Promise.resolve(snapshot) : bridge.snapshot(),
      bridge.loadLayout(),
      bridge.status(),
    ]);
    dispatch({ type: "patch", value: { snapshot: nextSnapshot, layout, status, error: null } });
  }, [bridge]);

  useEffect(() => {
    let active = true;
    void (async () => {
      try {
        const status = await bridge.status();
        if (!active) return;
        dispatch({ type: "patch", value: { status } });
        if (status.unlocked) await loadWorkspace();
      } catch (error) {
        if (active) dispatch({ type: "patch", value: { error: errorMessage(error) } });
      }
    })();
    let unlisten: (() => void) | undefined;
    void bridge.onVaultLocked(async () => {
      const status = await bridge.status();
      dispatch({ type: "locked", status });
    }).then((value) => { unlisten = value; });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [bridge, loadWorkspace]);

  useEffect(() => {
    if (!state.snapshot || !state.query.trim()) {
      dispatch({ type: "patch", value: { searchResults: [] } });
      return;
    }
    const timer = window.setTimeout(() => {
      void bridge.search(state.query).then(
        (searchResults) => dispatch({ type: "patch", value: { searchResults } }),
        (error) => dispatch({ type: "patch", value: { error: errorMessage(error) } }),
      );
    }, 140);
    return () => window.clearTimeout(timer);
  }, [bridge, state.query, state.snapshot]);

  const refresh = useCallback(async () => {
    const snapshot = await bridge.snapshot();
    dispatch({ type: "patch", value: { snapshot } });
    return snapshot;
  }, [bridge]);

  const lock = async () => {
    try {
      await perform(() => bridge.lock(), "仓库已锁定");
      const status = await bridge.status();
      dispatch({ type: "locked", status });
    } catch {}
  };

  const saveFolder = async (input: FolderInput, existing?: Folder) => {
    try {
      await perform(
        () => existing
          ? bridge.updateFolder(existing.id, existing.revision, input)
          : bridge.createFolder(input),
        existing ? "文件夹已更新" : "文件夹已创建",
      );
      await refresh();
      dispatch({ type: "patch", value: { entityDialog: null } });
    } catch {}
  };

  const saveConnection = async (input: ConnectionInput, existing?: ConnectionProfile) => {
    try {
      const value = await perform(
        () => existing
          ? bridge.updateConnection(existing.id, existing.revision, input)
          : bridge.createConnection(input),
        existing ? "连接已更新" : "连接已创建",
      );
      await refresh();
      openConnection(value.id);
      dispatch({ type: "patch", value: { entityDialog: null } });
    } catch {}
  };

  const openConnection = (id: string) => {
    dispatch({
      type: "patch",
      value: {
        selection: { kind: "connection", id },
        openTabs: state.openTabs.includes(id) ? state.openTabs : [...state.openTabs, id],
        activeTab: id,
      },
    });
  };

  const closeTab = (id: string) => {
    const openTabs = state.openTabs.filter((value) => value !== id);
    dispatch({
      type: "patch",
      value: {
        openTabs,
        activeTab: state.activeTab === id ? openTabs.at(-1) ?? null : state.activeTab,
      },
    });
  };

  const saveLayout = async (layout: WorkspaceLayout) => {
    dispatch({ type: "patch", value: { layout } });
    try {
      await bridge.saveLayout(layout);
    } catch (error) {
      dispatch({ type: "patch", value: { error: errorMessage(error) } });
    }
  };

  const beginResize = (axis: "left" | "right" | "bottom", event: React.PointerEvent) => {
    event.preventDefault();
    const origin = axis === "bottom" ? event.clientY : event.clientX;
    const initial = axis === "left" ? state.layout.leftWidth : axis === "right" ? state.layout.rightWidth : state.layout.bottomHeight;
    let latestLayout = state.layout;
    const move = (next: PointerEvent) => {
      const rawDelta = (axis === "bottom" ? next.clientY : next.clientX) - origin;
      const value = initial + (axis === "right" || axis === "bottom" ? -rawDelta : rawDelta);
      const bounded = axis === "left" ? Math.min(480, Math.max(180, value)) : axis === "right" ? Math.min(520, Math.max(220, value)) : Math.min(420, Math.max(100, value));
      const layout = { ...state.layout, [axis === "left" ? "leftWidth" : axis === "right" ? "rightWidth" : "bottomHeight"]: Math.round(bounded) };
      latestLayout = layout;
      dispatch({ type: "patch", value: { layout } });
    };
    const up = (next: PointerEvent) => {
      move(next);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      void bridge.saveLayout(latestLayout);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const nudgeLayout = (axis: "left" | "right" | "bottom", delta: number) => {
    const key = axis === "left" ? "leftWidth" : axis === "right" ? "rightWidth" : "bottomHeight";
    const limits = axis === "left" ? [180, 480] : axis === "right" ? [220, 520] : [100, 420];
    const layout = { ...state.layout, [key]: Math.min(limits[1], Math.max(limits[0], state.layout[key] + delta)) };
    void saveLayout(layout);
  };

  if (!state.status) {
    return <main className="loading-screen"><ThemeControl mode={themeMode} onChange={setThemeMode} floating /><div className="brand-mark"><span>&gt;_</span></div><span>FirstShell</span></main>;
  }

  if (!state.status.unlocked || !state.snapshot) {
    return (
      <>
        <ThemeControl mode={themeMode} onChange={setThemeMode} floating />
        <VaultGate
        status={state.status}
        busy={state.busy}
        error={state.error}
        onInitialize={async (password) => {
          try {
            const snapshot = await perform(() => bridge.initialize(password), "安全仓库已创建");
            await loadWorkspace(snapshot);
          } catch {}
        }}
        onUnlock={async () => {
          try {
            const snapshot = await perform(() => bridge.unlock(), "仓库已解锁");
            await loadWorkspace(snapshot);
          } catch {}
        }}
        onRecover={async (password) => {
          try {
            const snapshot = await perform(() => bridge.recover(password), "系统密钥已恢复");
            await loadWorkspace(snapshot);
          } catch {}
        }}
        />
      </>
    );
  }

  const snapshot = state.snapshot;
  const selectedFolder = state.selection?.kind === "folder"
    ? snapshot.folders.find((value) => value.id === state.selection?.id)
    : undefined;
  const selectedConnection = state.selection?.kind === "connection"
    ? snapshot.connections.find((value) => value.id === state.selection?.id)
    : undefined;
  const activeConnection = snapshot.connections.find((value) => value.id === state.activeTab);

  const deleteSelected = async () => {
    const label = selectedFolder?.name ?? selectedConnection?.name;
    if (!label || !window.confirm(`删除“${label}”？`)) return;
    try {
      const undoDelete: UndoDelete = selectedFolder
        ? { kind: "folder", id: selectedFolder.id, expectedRevision: selectedFolder.revision + 1, label }
        : { kind: "connection", id: selectedConnection!.id, expectedRevision: selectedConnection!.revision + 1, label };
      if (selectedFolder) await perform(() => bridge.deleteFolder(selectedFolder.id, selectedFolder.revision), "文件夹已删除");
      if (selectedConnection) await perform(() => bridge.deleteConnection(selectedConnection.id, selectedConnection.revision), "连接已删除");
      await refresh();
      const openTabs = state.openTabs.filter((id) => id !== undoDelete.id);
      dispatch({ type: "patch", value: { selection: null, openTabs, activeTab: state.activeTab === undoDelete.id ? openTabs.at(-1) ?? null : state.activeTab, undoDelete } });
    } catch {}
  };

  const undoDelete = async () => {
    const pending = state.undoDelete;
    if (!pending) return;
    try {
      const restored = pending.kind === "folder"
        ? await perform(() => bridge.restoreFolder(pending.id, pending.expectedRevision), `已恢复“${pending.label}”`)
        : await perform(() => bridge.restoreConnection(pending.id, pending.expectedRevision), `已恢复“${pending.label}”`);
      await refresh();
      dispatch({ type: "patch", value: { selection: { kind: pending.kind, id: restored.id }, undoDelete: null } });
    } catch {}
  };

  const layoutStyle = {
    "--left-panel": `${state.layout.leftWidth}px`,
    "--right-panel": `${state.layout.rightWidth}px`,
    "--bottom-panel": `${state.layout.bottomHeight}px`,
  } as React.CSSProperties;

  return (
    <main className="workspace" style={layoutStyle}>
      <header className="titlebar">
        <div className="title-brand"><div className="mini-mark">&gt;_</div><strong>FirstShell</strong><span>本地工作区</span></div>
        <div className="title-actions">
          {!state.status.keyringAvailable && <span className="status-warning">系统密钥不可用</span>}
          <ThemeControl mode={themeMode} onChange={setThemeMode} />
          <button className="icon-button" type="button" aria-label="备份与恢复" title="备份与恢复" onClick={() => dispatch({ type: "patch", value: { backupOpen: true, error: null } })}><Archive size={17} /></button>
          <button className="icon-button" type="button" aria-label="工作区配置" title="工作区配置" onClick={() => dispatch({ type: "patch", value: { settingsOpen: true, error: null } })}><Settings size={17} /></button>
          <button className="icon-button" type="button" aria-label="锁定仓库" title="锁定仓库" onClick={() => void lock()}><Lock size={17} /></button>
        </div>
      </header>

      <aside className="connections-panel" aria-label="连接">
        <div className="panel-header"><span>连接</span><div><button className="icon-button" type="button" aria-label="新建文件夹" title="新建文件夹" onClick={() => dispatch({ type: "patch", value: { entityDialog: { kind: "folder" }, error: null } })}><FolderPlus size={16} /></button><button className="icon-button" type="button" aria-label="新建连接" title="新建连接" onClick={() => dispatch({ type: "patch", value: { entityDialog: { kind: "connection" }, error: null } })}><Plus size={17} /></button></div></div>
        <label className="search-box"><Search size={15} aria-hidden="true" /><input aria-label="搜索工作区" placeholder="搜索" value={state.query} onChange={(event) => dispatch({ type: "patch", value: { query: event.target.value } })} /></label>
        <div className="connection-tree" role="tree" aria-label="连接树">
          {state.query.trim() ? state.searchResults.map((result) => (
            <button className="tree-row search-result" type="button" key={`${result.kind}-${result.id}`} onClick={() => result.kind === "connection" ? openConnection(result.id) : dispatch({ type: "patch", value: { selection: { kind: "folder", id: result.id } } })}>
              {result.kind === "folder" ? <FolderIcon size={15} /> : <Server size={15} />}<span><strong>{result.label}</strong><small>{result.detail}</small></span>
            </button>
          )) : (
            <TreeView snapshot={snapshot} selection={state.selection} onSelect={(selection) => dispatch({ type: "patch", value: { selection } })} onOpenConnection={openConnection} />
          )}
        </div>
      </aside>

      <PanelSeparator orientation="vertical" label="调整连接面板宽度" onPointerDown={(event) => beginResize("left", event)} onNudge={(delta) => nudgeLayout("left", delta)} />

      <section className="center-stack" aria-label="工作区">
        <div className="terminal-surface">
          <div className="tabbar" role="tablist" aria-label="会话标签">
            {state.openTabs.map((id) => {
              const connection = snapshot.connections.find((value) => value.id === id);
              if (!connection) return null;
              return <div className="session-tab" key={id}><button className="tab-select" role="tab" aria-selected={state.activeTab === id} type="button" onClick={() => dispatch({ type: "patch", value: { activeTab: id, selection: { kind: "connection", id } } })}><ConnectionIcon kind={connection.kind} /><span>{connection.name}</span></button><button className="tab-close" type="button" aria-label={`关闭 ${connection.name}`} title="关闭" onClick={() => closeTab(id)}><X size={13} /></button></div>;
            })}
            {state.openTabs.length === 0 && <div className="tab-placeholder">工作区</div>}
          </div>
          {activeConnection ? <ConnectionCanvas connection={activeConnection} /> : <WorkspaceOverview snapshot={snapshot} />}
        </div>
        {state.layout.bottomOpen && <>
          <PanelSeparator orientation="horizontal" label="调整活动面板高度" onPointerDown={(event) => beginResize("bottom", event)} onNudge={(delta) => nudgeLayout("bottom", delta)} />
          <section className="activity-panel" aria-label="活动">
            <div className="activity-header"><span><Activity size={14} />活动</span><button className="icon-button" type="button" aria-label="收起活动面板" title="收起" onClick={() => void saveLayout({ ...state.layout, bottomOpen: false })}><ChevronDown size={15} /></button></div>
            <div className="activity-list">{state.activities.map((item) => <div key={item.id}><time>{item.time}</time><span>{item.message}</span></div>)}{state.activities.length === 0 && <div className="empty-list">暂无活动</div>}</div>
          </section>
        </>}
      </section>

      <PanelSeparator orientation="vertical" label="调整详情面板宽度" onPointerDown={(event) => beginResize("right", event)} onNudge={(delta) => nudgeLayout("right", -delta)} />

      <aside className="inspector-panel" aria-label="详情">
        <div className="panel-header"><span>详情</span>{(selectedFolder || selectedConnection) && <div><button className="icon-button" type="button" aria-label="编辑所选项目" title="编辑" onClick={() => dispatch({ type: "patch", value: { entityDialog: selectedFolder ? { kind: "folder", value: selectedFolder } : { kind: "connection", value: selectedConnection }, error: null } })}><Edit3 size={15} /></button><button className="icon-button danger" type="button" aria-label="删除所选项目" title="删除" onClick={() => void deleteSelected()}><Trash2 size={15} /></button></div>}</div>
        <Inspector folder={selectedFolder} connection={selectedConnection} snapshot={snapshot} />
      </aside>

      <footer className="statusbar">
        <span><Circle size={7} fill="currentColor" />未连接</span>
        <span>{snapshot.connections.length} 个连接</span>
        {!state.layout.bottomOpen && <button type="button" onClick={() => void saveLayout({ ...state.layout, bottomOpen: true })}><Activity size={13} />活动</button>}
        {state.undoDelete && <button type="button" aria-label={`撤销删除 ${state.undoDelete.label}`} onClick={() => void undoDelete()}><Undo2 size={13} />撤销删除</button>}
        <span>M1</span>
      </footer>

      {state.entityDialog && <EntityDialog dialog={state.entityDialog} snapshot={snapshot} busy={state.busy} error={state.error} onClose={() => dispatch({ type: "patch", value: { entityDialog: null, error: null } })} onSaveFolder={saveFolder} onSaveConnection={saveConnection} />}
      {state.settingsOpen && <SettingsDialog snapshot={snapshot} busy={state.busy} error={state.error} onClose={() => dispatch({ type: "patch", value: { settingsOpen: false, error: null } })} onSaveProxy={async (input: ProxyInput, existing?: ProxyProfile) => { await perform(() => existing ? bridge.updateProxy(existing.id, existing.revision, input) : bridge.createProxy(input), existing ? "代理已更新" : "代理已创建"); await refresh(); }} onDeleteProxy={async (value) => { if (!window.confirm(`删除“${value.name}”？`)) return; await perform(() => bridge.deleteProxy(value.id, value.revision), "代理已删除"); await refresh(); }} onSaveTerminal={async (input: TerminalProfileInput, existing?: TerminalProfile) => { await perform(() => existing ? bridge.updateTerminal(existing.id, existing.revision, input) : bridge.createTerminal(input), existing ? "终端配置已更新" : "终端配置已创建"); await refresh(); }} onDeleteTerminal={async (value) => { if (!window.confirm(`删除“${value.name}”？`)) return; await perform(() => bridge.deleteTerminal(value.id, value.revision), "终端配置已删除"); await refresh(); }} />}
      {state.backupOpen && <BackupDialog busy={state.busy} error={state.error} onClose={() => dispatch({ type: "patch", value: { backupOpen: false, error: null } })} onExport={(password) => perform(() => bridge.exportWorkspace(password), "加密备份已导出")} onValidate={(fileName, password) => perform(() => bridge.validateImport(fileName, password), "备份验证通过")} onRestore={async (fileName, password) => { const result = await perform(() => bridge.restoreImport(fileName, password), "工作区已恢复"); await refresh(); return result; }} />}
    </main>
  );
}

function ConnectionIcon({ kind }: { kind: ConnectionProfile["kind"] }) {
  if (kind === "rdp") return <Monitor size={15} aria-hidden="true" />;
  if (kind === "local") return <Laptop size={15} aria-hidden="true" />;
  return <Server size={15} aria-hidden="true" />;
}

function TreeView({ snapshot, selection, onSelect, onOpenConnection }: { snapshot: WorkspaceSnapshot; selection: Selection; onSelect(value: Selection): void; onOpenConnection(id: string): void }) {
  const roots = snapshot.folders.filter((folder) => !folder.parentId);
  const rootConnections = snapshot.connections.filter((connection) => !connection.folderId);
  return <>{roots.map((folder) => <FolderNode key={folder.id} folder={folder} snapshot={snapshot} selection={selection} onSelect={onSelect} onOpenConnection={onOpenConnection} />)}{rootConnections.map((connection) => <ConnectionRow key={connection.id} value={connection} selected={selection?.kind === "connection" && selection.id === connection.id} onSelect={onOpenConnection} />)}{roots.length === 0 && rootConnections.length === 0 && <div className="empty-list">暂无连接</div>}</>;
}

function FolderNode({ folder, snapshot, selection, onSelect, onOpenConnection }: { folder: Folder; snapshot: WorkspaceSnapshot; selection: Selection; onSelect(value: Selection): void; onOpenConnection(id: string): void }) {
  const [expanded, setExpanded] = useReducer((value) => !value, true);
  const children = snapshot.folders.filter((value) => value.parentId === folder.id);
  const connections = snapshot.connections.filter((value) => value.folderId === folder.id);
  return <div role="treeitem" aria-expanded={expanded}><button className={`tree-row ${selection?.kind === "folder" && selection.id === folder.id ? "selected" : ""}`} type="button" onClick={() => { onSelect({ kind: "folder", id: folder.id }); setExpanded(); }} onKeyDown={(event) => { if (event.key === "ArrowRight" && !expanded) { event.preventDefault(); setExpanded(); } if (event.key === "ArrowLeft" && expanded) { event.preventDefault(); setExpanded(); } }}><span className="tree-expander" aria-hidden="true">{expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}</span><FolderIcon size={15} /><span>{folder.name}</span></button>{expanded && <div role="group" className="tree-group">{children.map((child) => <FolderNode key={child.id} folder={child} snapshot={snapshot} selection={selection} onSelect={onSelect} onOpenConnection={onOpenConnection} />)}{connections.map((connection) => <ConnectionRow key={connection.id} value={connection} selected={selection?.kind === "connection" && selection.id === connection.id} onSelect={onOpenConnection} />)}</div>}</div>;
}

function ConnectionRow({ value, selected, onSelect }: { value: ConnectionProfile; selected: boolean; onSelect(id: string): void }) {
  return <button className={`tree-row connection-row ${selected ? "selected" : ""}`} role="treeitem" type="button" onClick={() => onSelect(value.id)}><ConnectionIcon kind={value.kind} /><span><strong>{value.name}</strong><small>{value.kind === "local" ? "Local" : `${value.host}:${value.port}`}</small></span></button>;
}

function WorkspaceOverview({ snapshot }: { snapshot: WorkspaceSnapshot }) {
  return <div className="workspace-overview"><div className="overview-mark">&gt;_</div><h2>FirstShell</h2><div className="overview-stats"><span><Server size={16} />{snapshot.connections.length} 连接</span><span><FolderIcon size={16} />{snapshot.folders.length} 文件夹</span><span><HardDrive size={16} />本地仓库</span></div></div>;
}

function ConnectionCanvas({ connection }: { connection: ConnectionProfile }) {
  return <div className="connection-canvas"><div className="connection-identity"><ConnectionIcon kind={connection.kind} /><div><h2>{connection.name}</h2><span>{connection.kind === "local" ? "Local" : `${connection.username ? `${connection.username}@` : ""}${connection.host}:${connection.port}`}</span></div></div><div className="terminal-idle"><span>FirstShell</span><small>未连接</small></div></div>;
}

function Inspector({ folder, connection, snapshot }: { folder?: Folder; connection?: ConnectionProfile; snapshot: WorkspaceSnapshot }) {
  if (folder) return <div className="inspector-content"><div className="inspector-icon"><FolderIcon size={20} /></div><h2>{folder.name}</h2><dl><dt>上级</dt><dd>{snapshot.folders.find((value) => value.id === folder.parentId)?.name ?? "根目录"}</dd><dt>连接</dt><dd>{snapshot.connections.filter((value) => value.folderId === folder.id).length}</dd><dt>修订</dt><dd>{folder.revision}</dd></dl>{folder.notes && <p>{folder.notes}</p>}</div>;
  if (connection) return <div className="inspector-content"><div className="inspector-icon"><ConnectionIcon kind={connection.kind} /></div><h2>{connection.name}</h2><div className="kind-badge">{connection.kind.toUpperCase()}</div><dl><dt>主机</dt><dd>{connection.host || "Local"}</dd><dt>端口</dt><dd>{connection.port || "-"}</dd><dt>用户</dt><dd>{connection.username || "-"}</dd><dt>代理</dt><dd>{snapshot.proxies.find((value) => value.id === connection.proxyId)?.name ?? "Direct"}</dd><dt>终端</dt><dd>{snapshot.terminalProfiles.find((value) => value.id === connection.terminalProfileId)?.name ?? "默认"}</dd></dl>{connection.tags.length > 0 && <div className="tag-list">{connection.tags.map((tag) => <span key={tag}>{tag}</span>)}</div>}{connection.notes && <p>{connection.notes}</p>}</div>;
  return <div className="inspector-empty"><MoreHorizontal size={20} /><span>未选择项目</span></div>;
}

function PanelSeparator({ orientation, label, onPointerDown, onNudge }: { orientation: "vertical" | "horizontal"; label: string; onPointerDown(event: React.PointerEvent): void; onNudge(delta: number): void }) {
  return <div className={`panel-separator ${orientation}`} role="separator" aria-label={label} aria-orientation={orientation} tabIndex={0} onPointerDown={onPointerDown} onKeyDown={(event) => { const negative = orientation === "vertical" ? event.key === "ArrowLeft" : event.key === "ArrowDown"; const positive = orientation === "vertical" ? event.key === "ArrowRight" : event.key === "ArrowUp"; if (negative || positive) { event.preventDefault(); onNudge(positive ? 12 : -12); } }} />;
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<StrictMode><App /></StrictMode>);
