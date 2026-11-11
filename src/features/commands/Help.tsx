import { commands } from "./registry";
import Panel from "./Panel";

export default function Help({ onClose }: { onClose: () => void }) {
  return (
    <Panel title="Help" onClose={onClose}>
      <p className="cmd-lede">Type / in the message box to run a command or a skill.</p>
      <dl className="cmd-list">
        {commands.map((c) => (
          <div key={c.name} className="cmd-item">
            <dt>{c.name}</dt>
            <dd>{c.description}</dd>
          </div>
        ))}
      </dl>
    </Panel>
  );
}
