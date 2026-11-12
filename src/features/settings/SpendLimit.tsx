import { useCallback, useEffect, useId, useState } from "react";
import { setSpendLimit, spendLimit, type SpendLimit as Limit } from "../../lib/ipc";
import { toUiError } from "../chat/errors";
import Switch from "../skills/Switch";

/** "Spending limit for paid models": the daily limit, today's spend and the no-limit switch. */
export default function SpendLimit() {
  const [limit, setLimit] = useState<Limit | null>(null);
  const [field, setField] = useState("");
  const [error, setError] = useState<string | null>(null);
  const ids = useId();

  const load = useCallback(async () => {
    try {
      const l = await spendLimit();
      setLimit(l);
      setField(l.limit_usd === null ? "1.00" : String(l.limit_usd));
    } catch {
      // Nothing to show until the core answers.
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const apply = async (value: number | null) => {
    setError(null);
    try {
      await setSpendLimit(value);
      await load();
    } catch (e) {
      // The core's own refusal, word for word.
      setError(toUiError(e).message);
    }
  };

  const amount = Number(field);
  // Switching No limit off with nothing in the field goes back to the default, never to 0.
  const wanted = field.trim() === "" || !Number.isFinite(amount) ? 1 : amount;
  return (
    <div className="setting">
      <h2>Spending limit for paid models</h2>
      {limit && (
        <>
          <p className="setting-line">{`Spent $${limit.spent_today_usd.toFixed(2)} today (UTC day)`}</p>
          <div className="setting-row">
            <label htmlFor="spend-limit-field">Limit in dollars</label>
            <input
              id="spend-limit-field"
              className="setting-field"
              type="number"
              min="0.10"
              step="0.10"
              inputMode="decimal"
              value={field}
              onChange={(e) => setField(e.target.value)}
            />
            <button
              type="button"
              className="btn"
              disabled={field.trim() === "" || !Number.isFinite(amount)}
              onClick={() => void apply(amount)}
            >
              Save limit
            </button>
          </div>
          <div className="setting-row">
            <span id={`${ids}-label`}>No limit</span>
            <Switch
              label="No limit"
              labelledBy={`${ids}-label`}
              describedBy={`${ids}-desc`}
              checked={limit.limit_usd === null}
              onChange={(on) => void apply(on ? null : wanted)}
            />
          </div>
          <p className="setting-line" id={`${ids}-desc`}>Tasks on paid models will not pause for cost.</p>
          <p className="setting-line">Several chats working at once can each go one request past the limit.</p>
          {error && (
            <p className="setting-error" role="alert">
              {error}
            </p>
          )}
        </>
      )}
    </div>
  );
}
