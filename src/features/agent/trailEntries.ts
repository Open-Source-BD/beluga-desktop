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
  /** Unique among the live entries; set when it starts. */
  liveId?: number;
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

/** A stored call with no result and no live information: only its name is known. */
export function bareEntry(call: StoredToolCall, running: boolean): TrailEntry {
  return {
    callId: call.id,
    name: call.name,
    summary: call.name,
    status: running ? "running" : "not_run",
  };
}

/** A stored call and the result stored for it, if any. */
export interface Slot {
  call: StoredToolCall;
  entry: TrailEntry | null;
  /** Stable while the call moves from the live trail into its stored row. */
  key: string;
  /** Id of the stored tool row that answered it. */
  rowId: string | null;
}

export interface StoredTrail {
  /** Slots of every assistant row that has calls. */
  rows: Map<string, Slot[]>;
  /** Slots of the replies after the latest user message, in order: where live calls land. */
  task: Slot[];
  /** Key base of the task's lines. */
  taskKey: string;
  /** Only calls of this assistant row can still be running. */
  lastAssistantId: string | null;
}

/**
 * Pairs calls with results the way the core does: a tool row answers a call only when
 * it lies between that call's assistant row and the next row that is not a tool row;
 * inside that stretch by id, and a repeated id pairs in order. Any other tool row is an
 * orphan and is never shown.
 */
export function pairStored(messages: StoredMessage[]): StoredTrail {
  const rows = new Map<string, Slot[]>();
  let lastUser = -1;
  let lastAssistantId: string | null = null;
  messages.forEach((m, i) => {
    if (m.kind === "message" && m.role === "user") lastUser = i;
    if (m.kind === "message" && m.role === "assistant") lastAssistantId = m.id;
  });
  const taskKey = lastUser >= 0 ? messages[lastUser].id : "start";
  const task: Slot[] = [];
  messages.forEach((m, i) => {
    if (m.kind !== "message" || m.role !== "assistant" || m.tool_calls.length === 0) return;
    const answers: StoredMessage[] = [];
    for (let j = i + 1; j < messages.length && messages[j].kind === "tool"; j++) answers.push(messages[j]);
    const used = new Set<string>();
    const slots = m.tool_calls.map((call, n): Slot => {
      const row = answers.find((t) => !used.has(t.id) && t.tool_call_id === call.id && t.tool_meta);
      if (row) used.add(row.id);
      const meta = row?.tool_meta;
      return {
        call,
        rowId: row?.id ?? null,
        key: `${m.id}:${n}`,
        entry: meta
          ? { callId: call.id, name: meta.name, summary: meta.summary, status: meta.status, detail: meta.detail }
          : null,
      };
    });
    if (i > lastUser) slots.forEach((s) => { s.key = `${taskKey}:${task.length}`; task.push(s); });
    rows.set(m.id, slots);
  });
  return { rows, task, taskKey, lastAssistantId };
}

/**
 * Where the live calls land: each takes the first still unanswered call of the task with its
 * id, in order. Calls that are not stored yet are `rest`.
 */
export function matchLive(trail: StoredTrail, live: TrailEntry[]) {
  const matched = new Map<Slot, TrailEntry>();
  const rest: TrailEntry[] = [];
  let from = 0;
  for (const l of live) {
    const j = trail.task.findIndex((s, k) => k >= from && !s.entry && s.call.id === l.callId);
    if (j < 0) rest.push(l);
    else {
      matched.set(trail.task[j], l);
      from = j + 1;
    }
  }
  return { matched, rest };
}

/** Live entries whose result arrived in `fresh` stored tool rows (they leave the live list). */
export function answeredLive(trail: StoredTrail, live: TrailEntry[], freshRows: Set<string>): Set<TrailEntry> {
  const out = new Set<TrailEntry>();
  const slots = trail.task.filter((s) => s.rowId && freshRows.has(s.rowId));
  let from = 0;
  for (const l of live) {
    const j = slots.findIndex((s, k) => k >= from && s.call.id === l.callId);
    if (j >= 0) {
      out.add(l);
      from = j + 1;
    }
  }
  return out;
}
