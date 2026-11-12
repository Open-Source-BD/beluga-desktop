import { useEffect, useRef } from "react";

interface Props {
  onInstall: () => void;
  onCancel: () => void;
}

/** The notice shown before installing a pack that needs one (claude-red). */
export default function RedWarning({ onInstall, onCancel }: Props) {
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
      <div className="dialog wide" role="dialog" aria-modal="true" aria-labelledby="red-title">
        <h3 id="red-title">This pack is for authorised security testing</h3>
        <p>
          Claude-Red contains offensive security techniques. Use it only on systems you own or have written
          permission to test. Many models will refuse parts of it. It installs switched off.
        </p>
        <div className="dialog-actions">
          <button type="button" className="btn" ref={cancel} onClick={onCancel}>
            Cancel
          </button>
          <button type="button" className="btn danger" onClick={onInstall}>
            Install, switched off
          </button>
        </div>
      </div>
    </div>
  );
}
