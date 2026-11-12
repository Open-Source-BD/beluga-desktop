/** Shown over the bottom of the thread while the view is scrolled up; pressing it returns to the end. */
export default function JumpToLatest({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" className="jump-latest" onClick={onClick}>
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
        <path d="M12 5v14M6 13l6 6 6-6" />
      </svg>
      Jump to latest
    </button>
  );
}
