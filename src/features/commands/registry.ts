/** What a command can do; the Shell supplies the real behaviour. */
export interface CommandContext {
  newChat(): void;
  openModel(): void;
  openSkills(): void;
  openSettings(): void;
  showUsage(): void;
  showHelp(): void;
  /** `keep` names what the summary must preserve. */
  compact(keep?: string): void;
  exportChat(): void;
}

export interface Command {
  /** With its leading "/". */
  name: string;
  description: string;
  /** `rest` is any text typed after the command name. */
  run(ctx: CommandContext, rest: string): void;
}

export const commands: Command[] = [
  { name: "/new", description: "Start a new chat", run: (c) => c.newChat() },
  { name: "/model", description: "Switch the model for this chat", run: (c) => c.openModel() },
  { name: "/skills", description: "See and manage your skills", run: (c) => c.openSkills() },
  {
    name: "/usage",
    description: "Show free requests left and what this chat has cost",
    run: (c) => c.showUsage(),
  },
  {
    name: "/compact",
    description: "Shorten this chat to save requests and cost",
    run: (c, rest) => c.compact(rest || undefined),
  },
  { name: "/export", description: "Save this chat as a Markdown file", run: (c) => c.exportChat() },
  { name: "/settings", description: "Open Settings", run: (c) => c.openSettings() },
  {
    name: "/help",
    description: "List commands and keyboard shortcuts",
    run: (c) => c.showHelp(),
  },
];
