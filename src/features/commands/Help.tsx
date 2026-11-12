import { commands } from "./registry";
import Panel from "./Panel";

const SHORTCUTS: { keys: string; does: string }[] = [
  { keys: "Enter", does: "Send the message" },
  { keys: "Shift+Enter", does: "Add a new line" },
  { keys: "Cmd/Ctrl+Enter", does: "Send the message; with the Settings switch on, the only way to send" },
  { keys: "Up arrow", does: "In an empty message box, edit your last message" },
  { keys: "Esc", does: "Stop a reply that is running" },
  { keys: "/", does: "Open the menu of commands and skills" },
  { keys: "@", does: "Pick a file from the chat's folder" },
  { keys: "Cmd/Ctrl+N", does: "Open a new chat" },
];

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
      <p className="cmd-lede">Keyboard shortcuts</p>
      <dl className="cmd-list">
        {SHORTCUTS.map((s) => (
          <div key={s.keys} className="cmd-item">
            <dt>
              <kbd>{s.keys}</kbd>
            </dt>
            <dd>{s.does}</dd>
          </div>
        ))}
      </dl>
    </Panel>
  );
}
