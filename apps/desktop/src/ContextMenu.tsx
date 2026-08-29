import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";

export interface ContextMenuItem {
  label: string;
  icon: ReactNode;
  onSelect(): void;
  danger?: boolean;
  disabled?: boolean;
}

interface ContextMenuProps {
  x: number;
  y: number;
  label: string;
  items: ContextMenuItem[];
  onClose(): void;
}

export function ContextMenu({ x, y, label, items, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    const close = () => onClose();
    const closeOnEscape = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") close(); };
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [onClose]);

  const moveFocus = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
    if (!buttons.length) return;
    event.preventDefault();
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const offset = event.key === "ArrowDown" ? 1 : -1;
    buttons[(current + offset + buttons.length) % buttons.length]?.focus();
  };

  const left = Math.max(8, Math.min(x, window.innerWidth - 190));
  const estimatedHeight = Math.min(items.length * 33 + 12, window.innerHeight - 16);
  const top = Math.max(8, Math.min(y, window.innerHeight - estimatedHeight - 8));

  return createPortal(
    <div ref={menuRef} className="context-menu" role="menu" aria-label={label} style={{ left, top }} onKeyDown={moveFocus} onPointerDown={(event) => event.stopPropagation()}>
      {items.map((item) => <button key={item.label} className={item.danger ? "danger" : undefined} type="button" role="menuitem" disabled={item.disabled} onClick={() => { onClose(); item.onSelect(); }}>{item.icon}<span>{item.label}</span></button>)}
    </div>,
    document.body,
  );
}
