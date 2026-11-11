import type { KeyStatus } from "../../lib/ipc";
import { formatMoney, resetTimeText } from "./allowance";
import "./gauge.css";

interface Props {
  status: KeyStatus;
  monthSpend: number;
  usingFreeModel: boolean;
  /** The last refresh failed; the numbers shown are the last known ones. */
  refreshFailed?: boolean;
}

const MARKS = 50;
const LOW = 10;

export default function Gauge({ status, monthSpend, usingFreeModel, refreshFailed = false }: Props) {
  const spend = `Spent this month: ${formatMoney(monthSpend)}`;
  const failed = refreshFailed && <small>Could not refresh.</small>;

  if (!usingFreeModel) {
    const balance =
      status.limit_remaining_usd === null ? "no limit set" : formatMoney(status.limit_remaining_usd);
    return (
      <div className="gauge">
        <small>{spend}</small>
        <small>{`Balance: ${balance}`}</small>
        {failed}
      </div>
    );
  }

  const daily = status.free_daily;
  if (!daily) {
    return (
      <div className="gauge">
        <div>Free requests left: unknown</div>
        <small>{spend}</small>
        {failed}
      </div>
    );
  }

  const count = Math.min(daily.limit, MARKS);
  const perMark = daily.limit / count;
  const spentMarks = Math.min(count, Math.floor(daily.used / perMark));
  const summary = `${daily.remaining} of ${daily.limit} free requests left`;
  return (
    <div className={daily.remaining <= LOW ? "gauge low" : "gauge"} role="group" aria-label={summary}>
      <div>
        <b>{daily.remaining}</b> <span className="of">of {daily.limit} free requests left</span>
      </div>
      <div className="ticks" aria-hidden="true">
        {Array.from({ length: count }, (_, i) => (
          <i key={i} className={i < spentMarks ? "u" : undefined} />
        ))}
      </div>
      <small>{`Resets at ${resetTimeText()}.`}</small>
      <small>{spend}</small>
      {failed}
    </div>
  );
}
