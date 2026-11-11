import {
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type ChangeEvent,
  type KeyboardEvent,
  type ReactNode,
  type Ref,
  type TextareaHTMLAttributes,
} from "react";
import type { AttachmentInput } from "../../lib/ipc";
import { PlusIcon, SendIcon, StopIcon } from "../shell/icons";
import {
  CANNOT_READ_IMAGES,
  MAX_ATTACHMENTS,
  MAX_ATTACHMENTS_MESSAGE,
  readAttachment,
} from "./attachments";

/** Extra instructions that travel with a send, set by whoever called `submit`. */
export interface SendIntent {
  /** Activate this skill in the chat before sending. */
  skill?: string;
}

/** What menus use to edit the box and send from it; the Composer still owns the text. */
export interface ComposerControl {
  /** Replaces the text, with the caret at `caret` (default: the end). */
  setText(text: string, caret?: number): void;
  /** Sends `text` (default: the box) with the current attachments, as the Send button would. */
  submit(text?: string, intent?: SendIntent): Promise<void>;
  /** Adds a chip as a picked file would, with the same limits and refusal copy. */
  addAttachment(attachment: AttachmentInput): void;
  /** Shows `message` where attachment refusals appear. */
  refuse(message: string): void;
}

interface Props {
  /**
   * The draft is cleared once this returns or its promise resolves. If the
   * promise rejects, the text and attachments stay in the box; showing the
   * error is the handler's job.
   */
  onSend: (
    text: string,
    attachments: AttachmentInput[],
    intent?: SendIntent,
  ) => void | Promise<void>;
  /** Told the text and caret after every edit or caret move, for menus that follow the box. */
  onTextChange?: (text: string, caret: number) => void;
  /** Sees a key first; returning true means it was handled and the box ignores it. */
  onKeyIntercept?: (e: KeyboardEvent<HTMLTextAreaElement>) => boolean;
  controlRef?: Ref<ComposerControl>;
  /** Rendered just above the box (a popover menu), anchored to it. */
  menu?: ReactNode;
  /** Extra attributes for the text field (combobox wiring for a menu). */
  fieldAria?: TextareaHTMLAttributes<HTMLTextAreaElement>;
  onStop?: () => void;
  /** A reply is streaming in this chat: sending is blocked and the button becomes Stop. */
  streaming?: boolean;
  disabled?: boolean;
  /** No model chosen: the box is disabled and shows "Pick a model to start." */
  noModel?: boolean;
  /** The chat already has messages, so the placeholder reads "Reply". */
  hasMessages?: boolean;
  /** The selected model can read images; false refuses images. Unknown counts as yes. */
  acceptsImages?: boolean;
  slots?: { model?: ReactNode };
}

const MAX_HEIGHT = 200;

export default function Composer({
  onSend,
  onTextChange,
  onKeyIntercept,
  controlRef,
  menu,
  fieldAria,
  onStop,
  streaming = false,
  disabled = false,
  noModel = false,
  hasMessages = false,
  acceptsImages = true,
  slots,
}: Props) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<AttachmentInput[]>([]);
  const [refusals, setRefusals] = useState<string[]>([]);
  const [sending, setSending] = useState(false);
  const field = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const locked = disabled || noModel;
  const hasImage = files.some((f) => f.mime.startsWith("image/"));
  const imageBlocked = hasImage && !acceptsImages;
  const canSendText = (t: string) =>
    !locked && !streaming && !sending && !imageBlocked && (t.trim() !== "" || files.length > 0);
  const canSend = canSendText(text);
  const caretAt = useRef<number | null>(null);
  const lastCaret = useRef(0);

  const change = (next: string, caret: number) => {
    lastCaret.current = caret;
    setText(next);
    onTextChange?.(next, caret);
  };
  const notices =
    imageBlocked && !refusals.includes(CANNOT_READ_IMAGES)
      ? [...refusals, CANNOT_READ_IMAGES]
      : refusals;

  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  // A caret requested by `setText` is applied once the new text is in the field.
  useLayoutEffect(() => {
    if (caretAt.current !== null && field.current) {
      field.current.setSelectionRange(caretAt.current, caretAt.current);
      caretAt.current = null;
    }
  }, [text]);

  const submit = async (override?: string, intent?: SendIntent) => {
    const body = override ?? text;
    if (!canSendText(body)) return;
    setSending(true);
    try {
      await (intent ? onSend(body.trim(), files, intent) : onSend(body.trim(), files));
    } catch {
      // Not accepted: keep the draft so nothing typed or attached is lost.
      return;
    } finally {
      setSending(false);
    }
    change("", 0);
    setFiles([]);
    setRefusals([]);
  };

  const attach = async (e: ChangeEvent<HTMLInputElement>) => {
    const chosen = Array.from(e.target.files ?? []);
    e.target.value = "";
    const accepted: AttachmentInput[] = [];
    const refused: string[] = [];
    const refuse = (m: string) => {
      if (!refused.includes(m)) refused.push(m);
    };
    for (const file of chosen) {
      if (files.length + accepted.length >= MAX_ATTACHMENTS) {
        refuse(MAX_ATTACHMENTS_MESSAGE);
        continue;
      }
      const r = await readAttachment(file);
      if ("error" in r) refuse(r.error);
      else if (r.attachment.mime.startsWith("image/") && !acceptsImages) refuse(CANNOT_READ_IMAGES);
      else accepted.push(r.attachment);
    }
    setFiles((cur) => [...cur, ...accepted].slice(0, MAX_ATTACHMENTS));
    setRefusals(refused);
  };

  useImperativeHandle(controlRef, () => ({
    setText(next, caret = next.length) {
      caretAt.current = caret;
      change(next, caret);
    },
    submit,
    addAttachment(a) {
      if (files.length >= MAX_ATTACHMENTS) return setRefusals([MAX_ATTACHMENTS_MESSAGE]);
      if (a.mime.startsWith("image/") && !acceptsImages) return setRefusals([CANNOT_READ_IMAGES]);
      setFiles((cur) => [...cur, a].slice(0, MAX_ATTACHMENTS));
      setRefusals([]);
    },
    refuse: (message) => setRefusals([message]),
  }));

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!e.nativeEvent.isComposing && onKeyIntercept?.(e)) {
      e.preventDefault();
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void submit();
    }
  };

  const placeholder = noModel
    ? "Pick a model to start."
    : hasMessages
      ? "Reply"
      : "Describe a feature, paste an error, or ask a question";

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      {menu && <div className="composer-menu">{menu}</div>}
      {files.length > 0 && (
        <ul className="attach-chips" aria-label="Attachments">
          {files.map((f, i) => (
            <li key={`${f.name}-${i}`} className="attach-chip">
              <span className="attach-name">{f.name}</span>
              <button
                type="button"
                className="attach-remove"
                aria-label={`Remove ${f.name}`}
                onClick={() => setFiles((cur) => cur.filter((_, j) => j !== i))}
              >
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" aria-hidden="true" focusable="false">
                  <path d="M6 6l12 12M18 6 6 18" />
                </svg>
              </button>
            </li>
          ))}
        </ul>
      )}
      {notices.length > 0 && (
        <div className="attach-notices" role="alert">
          {notices.map((n) => (
            <p key={n}>{n}</p>
          ))}
        </div>
      )}
      <textarea
        ref={field}
        className="composer-field"
        aria-label="Message"
        rows={1}
        value={text}
        placeholder={placeholder}
        disabled={locked}
        onChange={(e) => change(e.target.value, e.target.selectionStart)}
        onSelect={(e) => {
          const caret = e.currentTarget.selectionStart;
          if (caret !== lastCaret.current) change(e.currentTarget.value, caret);
        }}
        onKeyDown={onKeyDown}
        {...fieldAria}
      />
      <div className="composer-controls">
        <input
          ref={picker}
          type="file"
          multiple
          hidden
          tabIndex={-1}
          aria-label="Attach files"
          onChange={attach}
        />
        <button
          type="button"
          className="chip-btn"
          disabled={locked}
          onClick={() => picker.current?.click()}
        >
          <PlusIcon size={13} />
          Attach
        </button>
        <div className="composer-slot">{slots?.model}</div>
        {streaming ? (
          <button
            type="button"
            className="send"
            aria-label="Stop"
            onClick={onStop}
          >
            <StopIcon />
          </button>
        ) : (
          <button
            type="submit"
            className="send"
            aria-label="Send"
            disabled={!canSend}
          >
            <SendIcon />
          </button>
        )}
      </div>
    </form>
  );
}
