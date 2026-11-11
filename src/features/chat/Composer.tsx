import {
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { SendIcon, StopIcon } from "../shell/icons";

interface Props {
  onSend: (text: string) => void;
  onStop?: () => void;
  /** A reply is streaming in this chat: sending is blocked and the button becomes Stop. */
  streaming?: boolean;
  disabled?: boolean;
  /** No model chosen: the box is disabled and shows "Pick a model to start." */
  noModel?: boolean;
  /** The chat already has messages, so the placeholder reads "Reply". */
  hasMessages?: boolean;
  slots?: { model?: ReactNode };
}

const MAX_HEIGHT = 200;

export default function Composer({
  onSend,
  onStop,
  streaming = false,
  disabled = false,
  noModel = false,
  hasMessages = false,
  slots,
}: Props) {
  const [text, setText] = useState("");
  const field = useRef<HTMLTextAreaElement>(null);
  const locked = disabled || noModel;
  const canSend = !locked && !streaming && text.trim() !== "";

  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  const submit = () => {
    if (!canSend) return;
    onSend(text.trim());
    setText("");
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      submit();
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
        submit();
      }}
    >
      <textarea
        ref={field}
        className="composer-field"
        aria-label="Message"
        rows={1}
        value={text}
        placeholder={placeholder}
        disabled={locked}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <div className="composer-controls">
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
