interface Props {
  /** Switches "all edits in this chat" off, so the next edit asks again. */
  onSwitchOff: () => void;
}

/** Shown while all edits are allowed in the chat; clicking it switches that off. */
export default function EditsChip({ onSwitchOff }: Props) {
  return (
    <div className="edits-chip-row">
      <button type="button" className="edits-chip" onClick={onSwitchOff}>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
          <path d="M12 20h9" />
          <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" />
        </svg>
        Edits allowed
      </button>
    </div>
  );
}
