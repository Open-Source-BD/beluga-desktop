import { useEffect, useRef, useState } from "react";
import type { StoredMessage } from "../../lib/ipc";
import { PencilIcon } from "../shell/icons";
import { copyLabel, useCopy } from "./useCopy";

interface Props {
  role: "user" | "assistant";
  message: StoredMessage;
  /** The newest reply: its row is always shown, and only it can be retried. */
  isLatest: boolean;
  /** A reply is running: Retry is hidden. */
  running?: boolean;
  /** Writes text to the clipboard; the default is `navigator.clipboard.writeText`. */
  onCopy?: (text: string) => Promise<void> | void;
  onRetry?: () => void;
  onRetryWithModel?: () => void;
  /** Absent, a user message shows no Edit button. */
  onEdit?: () => void;
}

/**
 * The row under a message. It stays in the page (so keyboard focus reaches it)
 * but is only seen on the latest reply, or while the pointer or focus is inside
 * the message; `data-shown` mirrors that for the stylesheet.
 */
export default function MessageActions({
  role,
  message,
  isLatest,
  running = false,
  onCopy,
  onRetry,
  onRetryWithModel,
  onEdit,
}: Props) {
  const { state, copy } = useCopy(onCopy);
  const [menu, setMenu] = useState(false);
  const [near, setNear] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const retry = useRef<HTMLButtonElement>(null);
  const canRetry = role === "assistant" && isLatest && !running && onRetry !== undefined;
  const shown = (role === "assistant" && isLatest) || near || menu;

  // The row follows the pointer and focus of the message that holds it.
  useEffect(() => {
    const message = root.current?.parentElement;
    if (!message) return;
    const on = () => setNear(true);
    const off = (e: Event) => {
      const to = (e as FocusEvent | MouseEvent).relatedTarget as Node | null;
      if (!to || !message.contains(to)) setNear(false);
    };
    message.addEventListener("mouseenter", on);
    message.addEventListener("mouseleave", off);
    message.addEventListener("focusin", on);
    message.addEventListener("focusout", off);
    return () => {
      message.removeEventListener("mouseenter", on);
      message.removeEventListener("mouseleave", off);
      message.removeEventListener("focusin", on);
      message.removeEventListener("focusout", off);
    };
  }, []);

  // A menu closes on a click elsewhere and on Esc.
  useEffect(() => {
    if (!menu) return;
    const away = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setMenu(false);
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [menu]);

  const choose = (action?: () => void) => {
    setMenu(false);
    action?.();
  };

  return (
    <div
      ref={root}
      className="msg-actions"
      data-shown={shown}
      onKeyDown={(e) => {
        if (e.key === "Escape" && menu) {
          e.stopPropagation();
          setMenu(false);
          retry.current?.focus();
        }
      }}
    >
      <button type="button" className="msg-act" onClick={() => void copy(message.text)}>
        {copyLabel(state, "Copy")}
      </button>
      {role === "user" && onEdit && (
        <button type="button" className="msg-act icon" aria-label="Edit message" title="Edit message" onClick={onEdit}>
          <PencilIcon size={12} />
        </button>
      )}
      {canRetry && (
        <span className="msg-retry">
          <button
            ref={retry}
            type="button"
            className="msg-act"
            aria-haspopup="menu"
            aria-expanded={menu}
            onClick={() => setMenu((open) => !open)}
          >
            Retry
          </button>
          {menu && (
            <div className="msg-menu" role="menu">
              <button type="button" role="menuitem" className="msg-menu-item" onClick={() => choose(onRetry)}>
                Retry
              </button>
              <button
                type="button"
                role="menuitem"
                className="msg-menu-item"
                onClick={() => choose(onRetryWithModel)}
              >
                Retry with another model…
              </button>
              <p className="msg-menu-note">Uses one request</p>
            </div>
          )}
        </span>
      )}
    </div>
  );
}
