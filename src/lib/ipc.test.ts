import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke, listen } = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class<T> {
    onmessage: (m: T) => void;
    constructor(onmessage: (m: T) => void) {
      this.onmessage = onmessage;
    }
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen }));

import * as ipc from "./ipc";

describe("ipc", () => {
  beforeEach(() => {
    invoke.mockReset().mockResolvedValue(undefined);
    listen.mockReset().mockResolvedValue(() => {});
  });

  // [call, command name, argument object or undefined]
  const cases: [string, () => Promise<unknown>, string, unknown][] = [
    ["authStatus", () => ipc.authStatus(), "auth_status", undefined],
    ["signInStart", () => ipc.signInStart(), "sign_in_start", undefined],
    ["signInCodeUrl", () => ipc.signInCodeUrl(), "sign_in_code_url", undefined],
    ["signInCancel", () => ipc.signInCancel(), "sign_in_cancel", undefined],
    ["signInWithCode", () => ipc.signInWithCode("c1"), "sign_in_with_code", { code: "c1" }],
    ["signInWithKey", () => ipc.signInWithKey("k"), "sign_in_with_key", { key: "k" }],
    [
      "resolveKeyStorage",
      () => ipc.resolveKeyStorage("session"),
      "resolve_key_storage",
      { choice: "session" },
    ],
    ["signOut", () => ipc.signOut(), "sign_out", undefined],
    ["listModels", () => ipc.listModels(true), "list_models", { force: true }],
    ["keyStatus", () => ipc.keyStatus(), "key_status", undefined],
    ["listChats", () => ipc.listChats(), "list_chats", undefined],
    ["getMessages", () => ipc.getMessages("c"), "get_messages", { chatId: "c" }],
    ["createChat", () => ipc.createChat("m/a"), "create_chat", { modelId: "m/a" }],
    ["renameChat", () => ipc.renameChat("c", "T"), "rename_chat", { id: "c", title: "T" }],
    ["deleteChat", () => ipc.deleteChat("c"), "delete_chat", { id: "c" }],
    ["chatUsage", () => ipc.chatUsage("c"), "chat_usage", { chatId: "c" }],
    ["monthSpend", () => ipc.monthSpend(), "month_spend", undefined],
    ["skillsStatus", () => ipc.skillsStatus(), "skills_status", undefined],
    ["listPacks", () => ipc.listPacks(), "list_packs", undefined],
    ["getSkill", () => ipc.getSkill("p:s"), "get_skill", { id: "p:s" }],
    ["setSkillOn", () => ipc.setSkillOn("p:s", true), "set_skill_on", { id: "p:s", on: true }],
    ["setPackOn", () => ipc.setPackOn("p", false), "set_pack_on", { id: "p", on: false }],
    ["skillFile", () => ipc.skillFile("p:s", "a.md"), "skill_file", { id: "p:s", file: "a.md" }],
    ["packLicenses", () => ipc.packLicenses("p"), "pack_licenses", { id: "p" }],
    [
      "openExternal",
      () => ipc.openExternal("https://github.com/o/r"),
      "plugin:opener|open_url",
      { url: "https://github.com/o/r" },
    ],
    ["stop", () => ipc.stop("c"), "stop", { chatId: "c" }],
    ["chatState", () => ipc.chatState("c", "m/a"), "chat_state", { chatId: "c", modelId: "m/a" }],
    [
      "activateSkill",
      () => ipc.activateSkill("c", "p:s", "m/a"),
      "activate_skill",
      { chatId: "c", skillId: "p:s", modelId: "m/a" },
    ],
    [
      "deactivateSkill",
      () => ipc.deactivateSkill("c", "p:s"),
      "deactivate_skill",
      { chatId: "c", skillId: "p:s" },
    ],
    [
      "chatFullness",
      () => ipc.chatFullness("c", "m/a", 12),
      "chat_fullness",
      { chatId: "c", modelId: "m/a", draftChars: 12 },
    ],
    [
      "chooseChatFolder",
      () => ipc.chooseChatFolder("c"),
      "choose_chat_folder",
      { chatId: "c" },
    ],
    ["approvedFolders", () => ipc.approvedFolders(), "approved_folders", undefined],
    [
      "setChatFolder",
      () => ipc.setChatFolder("c", "/home/me/proj"),
      "set_chat_folder",
      { chatId: "c", path: "/home/me/proj" },
    ],
    [
      "setChatFolder clears with null",
      () => ipc.setChatFolder("c", null),
      "set_chat_folder",
      { chatId: "c", path: null },
    ],
    [
      "folderFiles",
      () => ipc.folderFiles("c", "read"),
      "folder_files",
      { chatId: "c", query: "read" },
    ],
    [
      "attachFolderFile",
      () => ipc.attachFolderFile("c", "src/a.ts"),
      "attach_folder_file",
      { chatId: "c", path: "src/a.ts" },
    ],
    ["extraPacks", () => ipc.extraPacks(), "extra_packs", undefined],
    [
      "installExtraPack",
      () => ipc.installExtraPack("claude-red", true),
      "install_extra_pack",
      { id: "claude-red", acknowledged: true },
    ],
    ["removePack", () => ipc.removePack("vercel"), "remove_pack", { id: "vercel" }],
    [
      "previewAdd folder",
      () => ipc.previewAdd({ kind: "folder" }),
      "preview_add",
      { source: { kind: "folder" } },
    ],
    [
      "previewAdd zip",
      () => ipc.previewAdd({ kind: "zip" }),
      "preview_add",
      { source: { kind: "zip" } },
    ],
    [
      "previewAdd github",
      () => ipc.previewAdd({ kind: "github", url: "https://github.com/o/r" }),
      "preview_add",
      { source: { kind: "github", url: "https://github.com/o/r" } },
    ],
    ["confirmAdd", () => ipc.confirmAdd("t1"), "confirm_add", { token: "t1" }],
    ["cancelAdd", () => ipc.cancelAdd("t1"), "cancel_add", { token: "t1" }],
    ["removeSkill", () => ipc.removeSkill("mine"), "remove_skill", { id: "mine" }],
    [
      "answerApproval",
      () => ipc.answerApproval("c", "a1", { decision: "once" }),
      "answer_approval",
      { chatId: "c", approvalId: "a1", answer: { decision: "once" } },
    ],
    [
      "answerApproval decline with a note",
      () => ipc.answerApproval("c", "a1", { decision: "decline", note: "No" }),
      "answer_approval",
      { chatId: "c", approvalId: "a1", answer: { decision: "decline", note: "No" } },
    ],
    ["waitingApproval", () => ipc.waitingApproval("c"), "waiting_approval", { chatId: "c" }],
    ["taskState", () => ipc.taskState("c"), "task_state", { chatId: "c" }],
    [
      "revokeEdits",
      () => ipc.revokeEdits("c"),
      "revoke_edits",
      { chatId: "c" },
    ],
    [
      "backgroundOutput",
      () => ipc.backgroundOutput("c", "bg1"),
      "background_output",
      { chatId: "c", id: "bg1" },
    ],
    [
      "stopBackground",
      () => ipc.stopBackground("c", "bg1"),
      "stop_background",
      { chatId: "c", id: "bg1" },
    ],
    ["projectRules", () => ipc.projectRules(), "project_rules", undefined],
    [
      "removeProjectRule",
      () => ipc.removeProjectRule("/work/proj", "git status"),
      "remove_project_rule",
      { folder: "/work/proj", rule: "git status" },
    ],
    ["spendLimit", () => ipc.spendLimit(), "spend_limit", undefined],
    [
      "setSpendLimit",
      () => ipc.setSpendLimit(2.5),
      "set_spend_limit",
      { limitUsd: 2.5 },
    ],
    [
      "setSpendLimit clears with null",
      () => ipc.setSpendLimit(null),
      "set_spend_limit",
      { limitUsd: null },
    ],
    [
      "openAuthUrl",
      () => ipc.openAuthUrl("https://openrouter.ai/auth"),
      "plugin:opener|open_url",
      { url: "https://openrouter.ai/auth" },
    ],
  ];

  it.each(cases)("%s invokes the right command with camelCase keys", async (_, call, cmd, args) => {
    await call();
    expect(invoke).toHaveBeenCalledTimes(1);
    if (args === undefined) {
      expect(invoke).toHaveBeenCalledWith(cmd);
    } else {
      expect(invoke).toHaveBeenCalledWith(cmd, args);
    }
  });

  it("the web view has no function that switches all edits on", () => {
    expect("setEditsAllowed" in ipc).toBe(false);
    expect(Object.keys(ipc).filter((k) => /edits/i.test(k))).toEqual(["revokeEdits"]);
  });

  it("openExternal refuses a URL that is not on github.com", async () => {
    for (const url of ["https://evil.com/", "http://github.com/o/r", "https://github.com.evil.com/x", "javascript:1"]) {
      await expect(ipc.openExternal(url)).rejects.toThrow();
    }
    expect(invoke).not.toHaveBeenCalled();
  });

  it("sendMessage passes a channel that delivers engine events", async () => {
    const seen: ipc.EngineEvent[] = [];
    const att = { name: "a.txt", mime: "text/plain", data_base64: "aGk=" };
    const result: ipc.TurnResult = { status: "interrupted", reason: "stopped", error: null };
    invoke.mockResolvedValueOnce(result);
    await expect(ipc.sendMessage("c", "hi", [att], "m/a", (e) => seen.push(e))).resolves.toEqual(
      result,
    );
    expect(invoke).toHaveBeenCalledTimes(1);
    const [cmd, args] = invoke.mock.calls[0];
    expect(cmd).toBe("send_message");
    expect(Object.keys(args).sort()).toEqual(
      ["attachments", "chatId", "modelId", "onEvent", "text"].sort(),
    );
    expect(args).toMatchObject({ chatId: "c", text: "hi", attachments: [att], modelId: "m/a" });
    args.onEvent.onmessage({ type: "text_delta", text: "Hel" });
    expect(seen).toEqual([{ type: "text_delta", text: "Hel" }]);
  });

  it("retryTurn passes chat id, model id and a channel", async () => {
    const seen: ipc.EngineEvent[] = [];
    const result: ipc.TurnResult = {
      status: "failed",
      reason: null,
      error: { kind: "network", message: "x", retryable: true },
    };
    invoke.mockResolvedValueOnce(result);
    await expect(ipc.retryTurn("c", "m/a", (e) => seen.push(e))).resolves.toEqual(result);
    const [cmd, args] = invoke.mock.calls[0];
    expect(cmd).toBe("retry_turn");
    expect(Object.keys(args).sort()).toEqual(["chatId", "modelId", "onEvent", "regenerate"]);
    expect(args).toMatchObject({ chatId: "c", modelId: "m/a", regenerate: false });
    args.onEvent.onmessage({ type: "done" });
    expect(seen).toEqual([{ type: "done" }]);
  });

  it("retryTurn passes regenerate true when asked", async () => {
    invoke.mockResolvedValueOnce({ status: "done", reason: null, error: null });
    await ipc.retryTurn("c", "m/b", () => {}, true);
    const [cmd, args] = invoke.mock.calls[0];
    expect(cmd).toBe("retry_turn");
    expect(args).toMatchObject({ chatId: "c", modelId: "m/b", regenerate: true });
  });

  it("peekUserMessage passes chat and message ids and returns text and attachments", async () => {
    const peeked: ipc.PeekedMessage = {
      text: "hi",
      attachments: [{ name: "a.txt", mime: "text/plain", data_base64: "aGk=" }],
    };
    invoke.mockResolvedValueOnce(peeked);
    await expect(ipc.peekUserMessage("c", "m1")).resolves.toEqual(peeked);
    expect(invoke.mock.calls).toEqual([["peek_user_message", { chatId: "c", messageId: "m1" }]]);
  });

  it("takeUserMessage and rowsAfter pass chat and message ids", async () => {
    const taken: ipc.TakenMessage = {
      text: "hi",
      attachments: [{ name: "a.txt", mime: "text/plain", data_base64: "aGk=" }],
      removed: 2,
    };
    invoke.mockResolvedValueOnce(taken).mockResolvedValueOnce(2);
    await expect(ipc.takeUserMessage("c", "m1")).resolves.toEqual(taken);
    await expect(ipc.rowsAfter("c", "m1")).resolves.toBe(2);
    expect(invoke.mock.calls).toEqual([
      ["take_user_message", { chatId: "c", messageId: "m1" }],
      ["rows_after", { chatId: "c", messageId: "m1" }],
    ]);
  });

  it("compactChat passes keep (null when not given) and exportChatWithDialog passes only a suggested name", async () => {
    const result: ipc.TurnResult = { status: "done", reason: null, error: null };
    invoke.mockResolvedValue(result);
    await expect(ipc.compactChat("c", "m/a", "the plan")).resolves.toEqual(result);
    await ipc.compactChat("c", "m/a");
    await ipc.exportChatWithDialog("c", "My chat");
    expect(invoke.mock.calls).toEqual([
      ["compact_chat", { chatId: "c", modelId: "m/a", keep: "the plan" }],
      ["compact_chat", { chatId: "c", modelId: "m/a", keep: null }],
      ["export_chat_with_dialog", { chatId: "c", suggestedName: "My chat" }],
    ]);
  });

  it("onAuthFinished unwraps the event payload", async () => {
    const handler = vi.fn();
    await ipc.onAuthFinished(handler);
    expect(listen).toHaveBeenCalledWith("auth://finished", expect.any(Function));
    listen.mock.calls[0][1]({ payload: { ok: false, reason: "denied" } });
    expect(handler).toHaveBeenCalledWith({ ok: false, reason: "denied" });
  });

  it("onAuthRevoked listens to auth://revoked", async () => {
    const handler = vi.fn();
    await ipc.onAuthRevoked(handler);
    expect(listen).toHaveBeenCalledWith("auth://revoked", expect.any(Function));
    listen.mock.calls[0][1]({ payload: null });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("onBackgroundChanged listens to background-changed and hands over the chat id", async () => {
    const handler = vi.fn();
    await ipc.onBackgroundChanged(handler);
    expect(listen).toHaveBeenCalledWith("background-changed", expect.any(Function));
    listen.mock.calls[0][1]({ payload: "chat-1" });
    expect(handler).toHaveBeenCalledWith("chat-1");
  });

  it("getMessages returns stored messages with their kind and seq", async () => {
    const note: ipc.StoredMessage = {
      id: "n1", role: "system", text: "Note", attachments: [],
      interrupted: false, created_at: 1, kind: "note", seq: 7, tool_calls: [], tool_call_id: null, tool_meta: null,
    };
    invoke.mockResolvedValueOnce([note]);
    const [m] = await ipc.getMessages("c");
    expect(m.kind).toBe("note");
    expect(m.seq).toBe(7);
  });

  it("previewAdd resolves with null when the dialog is cancelled, else a preview", async () => {
    const preview: ipc.AddPreview = {
      token: "t1",
      skills: [
        {
          id: "mine", name: "mine", description: "Mine.", on: true,
          acts_on_computer: false, broken: null, body_chars: 5, available: true,
        },
      ],
      skipped: ["mine/big.md"],
    };
    invoke.mockResolvedValueOnce(null).mockResolvedValueOnce(preview);
    await expect(ipc.previewAdd({ kind: "zip" })).resolves.toBeNull();
    await expect(ipc.previewAdd({ kind: "folder" })).resolves.toEqual(preview);
  });

  it("extraPacks returns packs with installed and needs_warning", async () => {
    const pack: ipc.ExtraPack = {
      id: "claude-red", title: "Claude-Red", about: "a", license: "MIT",
      repo: "SnailSploit/Claude-Red", installed: false, needs_warning: true,
    };
    invoke.mockResolvedValueOnce([pack]);
    await expect(ipc.extraPacks()).resolves.toEqual([pack]);
  });

  it("chatState returns active skills with their flags", async () => {
    const state: ipc.ChatState = {
      active_skills: [
        { id: "p:s", name: "s", too_long: false, acts_on_computer: true, unavailable: true },
      ],
      folder: null,
      project: false,
      compacted: false,
    };
    invoke.mockResolvedValueOnce(state);
    await expect(ipc.chatState("c", "m/a")).resolves.toEqual(state);
  });
});

// ---------------------------------------------------------------------------
// The Rust side's JSON samples against the TypeScript types
// ---------------------------------------------------------------------------
//
// `src-tauri/tests/fixtures/ipc/*.json` holds one value per variant of every
// shape the interface receives or sends, written by the Rust test
// `the_ipc_fixtures_are_up_to_date`. The validators below are hand-written and
// typed against `ipc.ts` (a field missing from a spec is a compile error; a
// field the sample has and the spec lacks fails the test), so neither side can
// drift silently.

type Check = (v: unknown) => boolean;
type Spec<T> = { [K in keyof T]-?: Check };

const str: Check = (v) => typeof v === "string";
const num: Check = (v) => typeof v === "number" && Number.isFinite(v);
const int: Check = (v) => Number.isInteger(v);
const bool: Check = (v) => typeof v === "boolean";
const nul: Check = (v) => v === null;
const or =
  (...cs: Check[]): Check =>
  (v) =>
    cs.some((c) => c(v));
const list =
  (c: Check): Check =>
  (v) =>
    Array.isArray(v) && v.every(c);
const oneOf =
  (...vals: string[]): Check =>
  (v) =>
    typeof v === "string" && vals.includes(v);
const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** An object with exactly the keys of `spec`, each passing its check. */
function obj<T>(spec: Spec<T>): Check {
  const keys = Object.keys(spec).sort();
  return (v) =>
    isRecord(v) &&
    JSON.stringify(Object.keys(v).sort()) === JSON.stringify(keys) &&
    keys.every((k) => (spec as Record<string, Check>)[k](v[k]));
}

/** A union tagged by `tag`: one spec per variant (the variants of `T`, all of them). */
function tagged<T extends Record<K, string>, K extends string>(
  tag: K,
  specs: { [V in T[K]]: Spec<Extract<T, Record<K, V>>> },
): { check: Check; tags: string[] } {
  const tags = Object.keys(specs).sort();
  return {
    tags,
    check: (v) =>
      isRecord(v) &&
      typeof v[tag] === "string" &&
      tags.includes(v[tag] as string) &&
      obj((specs as Record<string, Spec<unknown>>)[v[tag] as string])(v),
  };
}

const toolStatuses: Record<ipc.ToolStatus, true> = { ok: true, error: true, declined: true, not_run: true };
const uiErrorKinds: Record<ipc.UiErrorKind, true> = {
  unauthorized: true,
  rate_limited: true,
  no_credit: true,
  model_unavailable: true,
  context_too_long: true,
  network: true,
  keychain_unavailable: true,
  daily_limit: true,
  other: true,
};
const pauseReasons: Record<ipc.PauseReason, true> = {
  keep_going_declined: true,
  no_free_requests: true,
  daily_limit: true,
  chat_full: true,
  going_in_circles: true,
  rate_limited: true,
};
const planStates: Record<ipc.PlanState, true> = { pending: true, in_progress: true, done: true };
const status = oneOf(...Object.keys(toolStatuses));

const uiError = obj<ipc.UiError>({
  kind: oneOf(...Object.keys(uiErrorKinds)),
  message: str,
  retryable: bool,
});
const planItem = obj<ipc.PlanItem>({ text: str, state: oneOf(...Object.keys(planStates)) });
const bgInfo = obj<ipc.BgInfo>({
  id: str,
  command: str,
  running: bool,
  exit_code: or(int, nul),
  started_at: int,
});
const approval = tagged<ipc.Approval, "kind">("kind", {
  command: {
    kind: str,
    id: str,
    command: str,
    folder: str,
    rule: or(str, nul),
    outside: bool,
    first_in_project: bool,
    background: bool,
  },
  edit: {
    kind: str,
    id: str,
    path: str,
    created: bool,
    removed: list(str),
    added: list(str),
    removed_total: int,
    added_total: int,
    total_lines: int,
  },
  secret: { kind: str, id: str, path: str },
  keep_going: {
    kind: str,
    id: str,
    requests: int,
    free_left: or(int, nul),
    spent_today_usd: or(num, nul),
  },
});
const answer = tagged<ipc.Answer, "decision">("decision", {
  once: { decision: str },
  always: { decision: str },
  all_edits: { decision: str },
  decline: { decision: str, note: or(str, nul) },
  message: { decision: str, text: str },
  keep_going: { decision: str },
  stop_here: { decision: str },
});
const engineEvent = tagged<ipc.EngineEvent, "type">("type", {
  text_delta: { type: str, text: str },
  usage: { type: str, requests: int, cost_usd: num },
  done: { type: str },
  interrupted: { type: str, reason: str },
  failed: { type: str, error: uiError },
  tool_started: { type: str, call_id: str, name: str, summary: str },
  tool_output: { type: str, call_id: str, text: str },
  tool_finished: { type: str, call_id: str, status, summary: str, detail: or(str, nul) },
  approval_needed: { type: str, approval: approval.check },
  approval_cleared: { type: str, id: str },
  plan_updated: { type: str, items: list(planItem) },
  waiting: { type: str, seconds: int },
  paused: { type: str, reason: oneOf(...Object.keys(pauseReasons)) },
  background_changed: { type: str },
  message_stored: { type: str },
});
const toolMeta = obj<ipc.ToolMeta>({ name: str, status, summary: str, detail: or(str, nul) });
const storedMessage = obj<ipc.StoredMessage>({
  id: str,
  role: oneOf("user", "assistant", "system", "tool"),
  text: str,
  attachments: list(obj<ipc.StoredAttachment>({ name: str, mime: str, data_base64: str })),
  interrupted: bool,
  created_at: int,
  kind: oneOf("message", "note", "tool"),
  seq: int,
  tool_calls: list(obj<ipc.StoredToolCall>({ id: str, name: str, arguments: str })),
  tool_call_id: or(str, nul),
  tool_meta: or(toolMeta, nul),
});
const turnResult = obj<ipc.TurnResult>({
  status: oneOf("done", "interrupted", "failed", "paused"),
  reason: or(str, nul),
  error: or(uiError, nul),
});
const taskState = obj<ipc.TaskState>({
  plan: list(planItem),
  edits_allowed: bool,
  background: list(bgInfo),
  project: bool,
  waiting: or(approval.check, nul),
  running: bool,
});
const chatState = obj<ipc.ChatState>({
  active_skills: list(
    obj<ipc.ActiveSkill>({ id: str, name: str, too_long: bool, acts_on_computer: bool, unavailable: bool }),
  ),
  folder: or(str, nul),
  project: bool,
  compacted: bool,
});
const spendLimit = obj<ipc.SpendLimit>({ limit_usd: or(num, nul), spent_today_usd: num });
const projectRules = obj<ipc.ProjectRules>({ folder: str, rules: list(str) });

const raw = import.meta.glob("../../src-tauri/tests/fixtures/ipc/*.json", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const fixtures = Object.fromEntries(
  Object.entries(raw).map(([path, text]) => [path.split("/").pop() as string, text]),
);
const sample = (name: string): unknown[] => {
  expect(Object.keys(fixtures), `${name} is missing`).toContain(name);
  const value: unknown = JSON.parse(fixtures[name]);
  expect(Array.isArray(value)).toBe(true);
  return value as unknown[];
};

describe("the Rust JSON samples", () => {
  const files: [string, Check, string[] | null][] = [
    ["approval.json", approval.check, approval.tags],
    ["answer.json", answer.check, answer.tags],
    ["engine_event.json", engineEvent.check, engineEvent.tags],
    ["task_state.json", taskState, null],
    ["turn_result.json", turnResult, null],
    ["ui_error.json", uiError, null],
    ["bg_info.json", bgInfo, null],
    ["stored_message.json", storedMessage, null],
    ["chat_state.json", chatState, null],
    ["spend_limit.json", spendLimit, null],
    ["project_rules.json", projectRules, null],
  ];

  it.each(files)("%s matches the TypeScript types", (name, check) => {
    const samples = sample(name);
    expect(samples.length).toBeGreaterThan(0);
    samples.forEach((v, i) => expect(check(v), `${name}[${i}]: ${JSON.stringify(v)}`).toBe(true));
  });

  it.each(files.filter(([, , tags]) => tags))(
    "%s has a sample of every variant the types declare",
    (name, _check, tags) => {
      const key = name === "answer.json" ? "decision" : name === "approval.json" ? "kind" : "type";
      const seen = new Set(sample(name).map((v) => (v as Record<string, string>)[key]));
      expect([...seen].sort()).toEqual(tags);
    },
  );

  it("covers every status of TurnResult and every kind of rows", () => {
    expect(sample("turn_result.json").map((v) => (v as ipc.TurnResult).status).sort()).toEqual([
      "done",
      "failed",
      "interrupted",
      "paused",
    ]);
    const rows = sample("stored_message.json") as ipc.StoredMessage[];
    expect(new Set(rows.map((m) => m.kind))).toEqual(new Set(["message", "note", "tool"]));
    expect(rows.some((m) => m.tool_calls.length > 0)).toBe(true);
    expect(rows.some((m) => m.tool_meta?.status === "declined")).toBe(true);
  });

  it("an Answer sample goes back to the core as it came", async () => {
    invoke.mockResolvedValue(undefined);
    for (const a of sample("answer.json") as ipc.Answer[]) {
      await ipc.answerApproval("c", "a1", a);
      expect(invoke).toHaveBeenLastCalledWith("answer_approval", { chatId: "c", approvalId: "a1", answer: a });
    }
  });

  it("the sample files use LF line endings", () => {
    for (const [name, text] of Object.entries(fixtures)) {
      expect(text.includes("\r"), name).toBe(false);
      expect(text.endsWith("\n"), name).toBe(true);
    }
  });

  it("the validators refuse a field that is missing, extra or of the wrong type", () => {
    expect(approval.check({ kind: "secret", id: "a", path: ".env" })).toBe(true);
    expect(approval.check({ kind: "secret", id: "a" })).toBe(false);
    expect(approval.check({ kind: "secret", id: "a", path: ".env", extra: 1 })).toBe(false);
    expect(approval.check({ kind: "secret", id: 1, path: ".env" })).toBe(false);
    expect(approval.check({ kind: "other", id: "a" })).toBe(false);
    expect(answer.check({ decision: "decline" })).toBe(false);
  });
});
