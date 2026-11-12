import type { StoredMessage, StoredToolCall, ToolStatus } from "../../lib/ipc";

/** "running" while the call is in progress, then how it ended. */
export type TrailStatus = "running" | ToolStatus;

/** One tool call as the conversation shows it. Text comes from the core, already sanitised. */
export interface TrailEntry {
  callId: string;
  name: string;
  summary: string;
  status: TrailStatus;
  detail?: string | null;
  /** What a running command has printed so far (the last 20,000 characters). */
  output?: string;
}

/** Output kept in memory per running line. */
export const OUTPUT_KEEP = 20_000;

/** The only words the interface adds to the core's summary; none for a line that went well. */
export function stateWord(status: TrailStatus): string | null {
  switch (status) {
    case "running":
      return "running";
    case "error":
      return "failed";
    case "declined":
      return "declined";
    case "not_run":
      return "not run";
    default:
      return null;
  }
}

/** Calls that have a stored result, as entries. Stable while `messages` is. */
export function resolvedEntries(messages: StoredMessage[]): Map<string, TrailEntry> {
  const calls = new Set<string>();
  for (const m of messages) for (const c of m.tool_calls) calls.add(c.id);
  const out = new Map<string, TrailEntry>();
  for (const m of messages) {
    // A result whose call is in no assistant row (an orphan) is never shown.
    if (m.kind !== "tool" || !m.tool_call_id || !m.tool_meta || !calls.has(m.tool_call_id)) continue;
    out.set(m.tool_call_id, {
      callId: m.tool_call_id,
      name: m.tool_meta.name,
      summary: m.tool_meta.summary,
      status: m.tool_meta.status,
      detail: m.tool_meta.detail,
    });
  }
  return out;
}

/** A stored call with no result and no live information: only its name is known. */
export function bareEntry(call: StoredToolCall, running: boolean): TrailEntry {
  return {
    callId: call.id,
    name: call.name,
    summary: call.name,
    status: running ? "running" : "not_run",
  };
}
