import type { ChatUsage } from "../../lib/ipc";
import { formatChatUsage, formatMoney } from "../gauge/allowance";
import Panel from "./Panel";

interface Props {
  onClose: () => void;
  freeLeft: number | null;
  freeLimit: number | null;
  /** null when no chat is open. */
  chat: ChatUsage | null;
  monthSpend: number;
}

export default function Usage({ onClose, freeLeft, freeLimit, chat, monthSpend }: Props) {
  return (
    <Panel title="Usage" onClose={onClose}>
      <dl className="cmd-list">
        <div className="cmd-item">
          <dt>Free requests left today</dt>
          <dd>{freeLeft === null || freeLimit === null ? "unknown" : `${freeLeft} of ${freeLimit}`}</dd>
        </div>
        <div className="cmd-item">
          <dt>This chat</dt>
          <dd>{chat ? formatChatUsage(chat) : "No chat open"}</dd>
        </div>
        <div className="cmd-item">
          <dt>Spent this month</dt>
          <dd>{formatMoney(monthSpend)}</dd>
        </div>
      </dl>
    </Panel>
  );
}
