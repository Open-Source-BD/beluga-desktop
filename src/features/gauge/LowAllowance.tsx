import type { ModelInfo } from "../../lib/ipc";
import { formatPrice } from "../models/defaultModel";
import { cheapestPaidTools, resetTimeText } from "./allowance";
import "./gauge.css";

interface Props {
  models: ModelInfo[];
  onPick: (modelId: string) => void;
  /** Only the cheapest paid tool models, without the sentences about free requests (a task going in circles). */
  modelsOnly?: boolean;
}

export default function LowAllowance({ models, onPick, modelsOnly = false }: Props) {
  const options = cheapestPaidTools(models);
  return (
    <div className="low-allowance">
      {!modelsOnly && <p>{`You have used today's free requests. They reset at ${resetTimeText()}.`}</p>}
      {options.length > 0 && (
        <ul>
          {options.map((m) => (
            <li key={m.id}>
              <button type="button" className="btn" onClick={() => onPick(m.id)}>
                <span>{m.name}</span>
                <span className="price">{formatPrice(m)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {!modelsOnly && <p>A one-time $10 top-up on OpenRouter raises the free limit to 1,000 requests a day.</p>}
    </div>
  );
}
