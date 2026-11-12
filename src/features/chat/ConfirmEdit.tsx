import { useEffect, useRef } from "react";

interface Props {
  /** How many messages after the edited one will be removed. */
  count: number;
  onConfirm: () => void;
  onCancel: () => void;
}

export default function ConfirmEdit({ count, onConfirm, onCancel }: Props) {
  const cancel = useRef<HTMLButtonElement>(null);
  // Focus the safe choice first.
  useEffect(() => cancel.current?.focus(), []);
  return (
    <div
      className="scrim"
      onKeyDown={(e) => {
        if (e.key === "Escape") onCancel();
      }}
    >
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-edit-title">
        <p id="confirm-edit-title">{`Editing this message removes the ${count} messages after it.`}</p>
        <div className="dialog-actions">
          <button type="button" className="btn" ref={cancel} onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="btn danger" onClick={onConfirm}>
            Edit and remove
          </button>
        </div>
      </div>
    </div>
  );
}
