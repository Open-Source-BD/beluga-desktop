import { useEffect, useRef } from "react";

interface Props {
  onDelete: () => void;
  onKeep: () => void;
}

export default function ConfirmDelete({ onDelete, onKeep }: Props) {
  const keep = useRef<HTMLButtonElement>(null);
  // Focus the safe choice first.
  useEffect(() => keep.current?.focus(), []);
  return (
    <div
      className="scrim"
      onKeyDown={(e) => {
        if (e.key === "Escape") onKeep();
      }}
    >
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-delete-title"
      >
        <p id="confirm-delete-title">Delete this chat? This cannot be undone.</p>
        <div className="dialog-actions">
          <button type="button" className="btn" ref={keep} onClick={onKeep}>
            Keep chat
          </button>
          <button type="button" className="btn danger" onClick={onDelete}>
            Delete chat
          </button>
        </div>
      </div>
    </div>
  );
}
