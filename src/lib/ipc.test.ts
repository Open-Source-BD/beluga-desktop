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

  it("compactChat passes keep (null when not given) and exportChat passes the path", async () => {
    const result: ipc.TurnResult = { status: "done", reason: null, error: null };
    invoke.mockResolvedValue(result);
    await expect(ipc.compactChat("c", "m/a", "the plan")).resolves.toEqual(result);
    await ipc.compactChat("c", "m/a");
    await ipc.exportChat("c", "/tmp/chat.md");
    expect(invoke.mock.calls).toEqual([
      ["compact_chat", { chatId: "c", modelId: "m/a", keep: "the plan" }],
      ["compact_chat", { chatId: "c", modelId: "m/a", keep: null }],
      ["export_chat", { chatId: "c", path: "/tmp/chat.md" }],
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

  it("getMessages returns stored messages with their kind and seq", async () => {
    const note: ipc.StoredMessage = {
      id: "n1", role: "system", text: "Note", attachments: [],
      interrupted: false, created_at: 1, kind: "note", seq: 7,
    };
    invoke.mockResolvedValueOnce([note]);
    const [m] = await ipc.getMessages("c");
    expect(m.kind).toBe("note");
    expect(m.seq).toBe(7);
  });

  it("chatState returns active skills with their flags", async () => {
    const state: ipc.ChatState = {
      active_skills: [
        { id: "p:s", name: "s", too_long: false, acts_on_computer: true, unavailable: true },
      ],
      folder: null,
      compacted: false,
    };
    invoke.mockResolvedValueOnce(state);
    await expect(ipc.chatState("c", "m/a")).resolves.toEqual(state);
  });
});
