import { AlertTriangle, Trash2, X } from "lucide-react";
import { createPortal } from "react-dom";

interface ConfirmDialogProps {
  title: string;
  description: string;
  busy?: boolean;
  onClose(): void;
  onConfirm(): Promise<void> | void;
}

export function ConfirmDialog({ title, description, busy = false, onClose, onConfirm }: ConfirmDialogProps) {
  return createPortal(
    <div className="modal-backdrop credential-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && !busy && onClose()}>
      <section className="modal confirm-modal" role="alertdialog" aria-modal="true" aria-labelledby="confirm-dialog-title" aria-describedby="confirm-dialog-description">
        <header className="modal-header">
          <div className="credential-title"><span className="credential-icon warning"><AlertTriangle size={18} /></span><h2 id="confirm-dialog-title">{title}</h2></div>
          <button className="icon-button" type="button" aria-label="关闭" title="关闭" disabled={busy} onClick={onClose}><X size={17} /></button>
        </header>
        <p id="confirm-dialog-description" className="confirm-description">{description}</p>
        <footer className="modal-actions">
          <button type="button" disabled={busy} onClick={onClose}>取消</button>
          <button className="danger-command" type="button" disabled={busy} onClick={() => void onConfirm()}><Trash2 size={16} /><span>{busy ? "删除中…" : "删除"}</span></button>
        </footer>
      </section>
    </div>,
    document.body,
  );
}
