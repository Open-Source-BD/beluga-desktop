import type { Row } from "../menu/match";
import { commands } from "./registry";

export type SlashResolution =
  | { kind: "skill"; id: string; rest: string }
  | { kind: "command"; name: string; rest: string }
  | { kind: "plain" };

/**
 * Reads a message that starts with "/": when its first word is exactly an
 * available skill id (case-sensitive) or a command name (any case), it is a
 * skill or command and `rest` is what follows. A command that takes no text
 * is one only when typed alone ("/help me fix this" is a message). Anything
 * else is plain text.
 */
export function resolveSlash(text: string, skills: Row[]): SlashResolution {
  if (!text.startsWith("/")) return { kind: "plain" };
  const m = /^(\S+)\s*([\s\S]*)$/.exec(text);
  if (!m) return { kind: "plain" };
  const [, word, rest] = m;
  const skill = skills.find((r) => r.name === word);
  if (skill) return { kind: "skill", id: skill.id, rest };
  const command = commands.find((c) => c.name === word.toLowerCase());
  if (command && (rest === "" || command.takesText)) return { kind: "command", name: command.name, rest };
  return { kind: "plain" };
}
