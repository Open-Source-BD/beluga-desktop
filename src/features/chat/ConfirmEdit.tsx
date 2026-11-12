import { useEffect, useRef } from "react";

interface Props {
  /** How many messages after the edited one will be removed. */
  count: number;
  /** Tool rows follow the message in a project chat: what they did stays done. */
  toolsAfter?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export default function ConfirmEdit({ count, toolsAfter = false, onConfirm, onCancel }: Props) {
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
        {toolsAfter && <p>Files and commands already changed on your computer stay as they are.</p>}
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
