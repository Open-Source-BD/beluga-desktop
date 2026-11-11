import { useEffect, useRef, type ReactNode } from "react";
import "./commands.css";

interface Props {
  title: string;
  onClose: () => void;
  children: ReactNode;
}

/** A small panel above the message box: closes with its button or Esc. */
export default function Panel({ title, onClose, children }: Props) {
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => close.current?.focus(), []);
  return (
    <div
      className="cmd-panel"
      role="dialog"
      aria-label={title}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <div className="cmd-panel-hd">
        <b>{title}</b>
        <button ref={close} type="button" className="cmd-close" aria-label="Close" onClick={onClose}>
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" aria-hidden="true" focusable="false">
            <path d="M6 6l12 12M18 6 6 18" />
          </svg>
        </button>
      </div>
      {children}
    </div>
  );
}
