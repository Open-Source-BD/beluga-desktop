const SHOWN = 80;

interface Props {
  text: string;
  onRemove: () => void;
}

/** The message waiting for the running reply to finish. */
export default function Queued({ text, onRemove }: Props) {
  return (
    <div className="queued">
      <span className="queued-text">{`Queued: ${text.slice(0, SHOWN)}`}</span>
      <button type="button" className="queued-remove" aria-label="Remove queued message" onClick={onRemove}>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" aria-hidden="true" focusable="false">
          <path d="M6 6l12 12M18 6 6 18" />
        </svg>
      </button>
    </div>
  );
}
