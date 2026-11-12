/** How full the chat is. From 70 percent it turns amber and points at /compact. */
export const FULL_FROM = 70;

export default function Fullness({ percent }: { percent: number }) {
  const high = percent >= FULL_FROM;
  return (
    <span className={high ? "fullness high" : "fullness"}>
      {`Chat is ${percent}% full`}
      {high && <span className="fullness-hint">Use /compact to shorten it</span>}
    </span>
  );
}
