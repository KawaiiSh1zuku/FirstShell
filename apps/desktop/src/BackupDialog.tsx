import { ArchiveRestore, Download, ShieldCheck, X } from "lucide-react";
import { FormEvent, useState } from "react";
import type { ExportReceipt, ImportSummary, RestoreReceipt } from "./types";

interface BackupDialogProps {
  busy: boolean;
  error: string | null;
  onClose(): void;
  onExport(password: string): Promise<ExportReceipt>;
  onValidate(fileName: string, password: string): Promise<ImportSummary>;
  onRestore(fileName: string, password: string): Promise<RestoreReceipt>;
}

export function BackupDialog(props: BackupDialogProps) {
  const [tab, setTab] = useState<"export" | "import">("export");
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [fileName, setFileName] = useState("");
  const [receipt, setReceipt] = useState<ExportReceipt | null>(null);
  const [summary, setSummary] = useState<ImportSummary | null>(null);
  const [restored, setRestored] = useState<RestoreReceipt | null>(null);

  const exportBackup = async (event: FormEvent) => {
    event.preventDefault();
    if (password !== confirmation) return;
    try { setReceipt(await props.onExport(password)); } catch {}
  };
  const validate = async (event: FormEvent) => {
    event.preventDefault();
    try { setSummary(await props.onValidate(fileName, password)); } catch {}
  };

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && props.onClose()}>
      <section className="modal backup-modal" role="dialog" aria-modal="true" aria-labelledby="backup-title">
        <header className="modal-header"><h2 id="backup-title">备份与恢复</h2><button className="icon-button" type="button" aria-label="关闭" title="关闭" onClick={props.onClose}><X size={17} /></button></header>
        <div className="segmented backup-tabs" aria-label="备份操作"><button type="button" aria-pressed={tab === "export"} onClick={() => { setTab("export"); setPassword(""); }}>加密导出</button><button type="button" aria-pressed={tab === "import"} onClick={() => { setTab("import"); setPassword(""); }}>验证恢复</button></div>
        {tab === "export" ? (
          <form className="entity-form" onSubmit={exportBackup}>
            <label><span>导出口令</span><input autoFocus type="password" minLength={12} maxLength={1024} required value={password} onChange={(event) => setPassword(event.target.value)} /></label>
            <label><span>确认导出口令</span><input type="password" minLength={12} maxLength={1024} required aria-invalid={confirmation.length > 0 && password !== confirmation} value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
            {receipt && <div className="result-block"><ShieldCheck size={17} /><div><strong>{receipt.fileName}</strong><span>{receipt.bytes.toLocaleString()} bytes</span></div></div>}
            {props.error && <div className="inline-error" role="alert">{props.error}</div>}
            <footer className="modal-actions"><button className="primary-command" type="submit" disabled={props.busy || password !== confirmation}><Download size={16} /><span>导出</span></button></footer>
          </form>
        ) : (
          <form className="entity-form" onSubmit={validate}>
            <label><span>备份文件名</span><input autoFocus required pattern="[^/\\]+\.firstshell-backup" value={fileName} onChange={(event) => { setFileName(event.target.value); setSummary(null); }} /></label>
            <label><span>导出口令</span><input type="password" minLength={1} maxLength={1024} required value={password} onChange={(event) => { setPassword(event.target.value); setSummary(null); }} /></label>
            {summary && <div className="result-block"><ShieldCheck size={17} /><div><strong>{summary.connections} 个连接</strong><span>{summary.folders} 个文件夹 · {summary.proxies} 个代理 · {summary.terminalProfiles} 个终端配置 · {summary.snippets} 个片段</span></div></div>}
            {restored && <div className="result-block success"><ArchiveRestore size={17} /><div><strong>恢复完成</strong><span>{restored.recoveryFileName}</span></div></div>}
            {props.error && <div className="inline-error" role="alert">{props.error}</div>}
            <footer className="modal-actions"><button type="submit" disabled={props.busy}>验证</button><button className="primary-command" type="button" disabled={props.busy || !summary} onClick={async () => { try { setRestored(await props.onRestore(fileName, password)); } catch {} }}><ArchiveRestore size={16} /><span>恢复</span></button></footer>
          </form>
        )}
      </section>
    </div>
  );
}
