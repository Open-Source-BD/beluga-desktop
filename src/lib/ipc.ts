// Typed wrappers for every Rust command and event. Field names match the
// Rust structs exactly (snake_case, as serde emits them). Command argument
// keys are camelCase: Tauri maps them to the Rust snake_case parameters.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type StorageKind = "keychain" | "session" | "file";
export type StorageChoice = "session" | "file";

export interface AuthStatus {
  signed_in: boolean;
  storage: StorageKind | null;
}

export interface AuthUrl {
  auth_url: string;
}

export type AuthFinishedReason =
  | "denied"
  | "timed_out"
  | "cancelled"
  | "keychain_unavailable"
  | "failed";

export interface AuthFinished {
  ok: boolean;
  reason: AuthFinishedReason | null;
}

export type UiErrorKind =
  | "unauthorized"
  | "rate_limited"
  | "no_credit"
  | "model_unavailable"
  | "context_too_long"
  | "network"
  | "keychain_unavailable"
  | "other";

/** The rejection value of every command that can fail. */
export interface UiError {
  kind: UiErrorKind;
  message: string;
  retryable: boolean;
}

export interface ModelInfo {
  id: string;
  name: string;
  prompt_usd_per_mtok: number;
  completion_usd_per_mtok: number;
  is_free: boolean;
  supports_tools: boolean;
  accepts_images: boolean;
  context_length: number;
}

export interface FreeDaily {
  used: number;
  limit: number;
  remaining: number;
}

export interface KeyStatus {
  is_free_tier: boolean;
  free_daily: FreeDaily | null;
  usage_usd: number;
  limit_remaining_usd: number | null;
}

export interface ChatSummary {
  id: string;
  title: string;
  model_id: string;
  updated_at: number;
}

export interface StoredAttachment {
  name: string;
  mime: string;
  data_base64: string;
}

export type AttachmentInput = StoredAttachment;

export interface StoredMessage {
  id: string;
  role: "user" | "assistant" | "system";
  text: string;
  attachments: StoredAttachment[];
  interrupted: boolean;
  created_at: number;
  kind: "message" | "note";
  seq: number;
}

export interface ChatUsage {
  requests: number;
  cost_usd: number;
}

/** `reason` is "stopped" when the user stopped the reply, otherwise a UiError kind. */
export type EngineEvent =
  | { type: "text_delta"; text: string }
  | { type: "usage"; requests: number; cost_usd: number }
  | { type: "done" }
  | { type: "interrupted"; reason: "stopped" | UiErrorKind }
  | { type: "failed"; error: UiError };

/**
 * How a turn that ran ended. The command resolves with it only after the
 * reply and usage are stored. `reason` is set for "interrupted" ("stopped" or
 * a UiError kind), `error` for "failed". A rejection means the turn never ran.
 */
export interface TurnResult {
  status: "done" | "interrupted" | "failed";
  reason: string | null;
  error: UiError | null;
}

export const AUTH_FINISHED = "auth://finished";
export const AUTH_REVOKED = "auth://revoked";

export const authStatus = () => invoke<AuthStatus>("auth_status");

/** Starts the browser sign-in; the result arrives through `onAuthFinished`. */
export const signInStart = () => invoke<AuthUrl>("sign_in_start");

/** An auth URL for the paste-a-code path. Open it with `openAuthUrl`. */
export const signInCodeUrl = () => invoke<AuthUrl>("sign_in_code_url");

export const signInCancel = () => invoke<void>("sign_in_cancel");

export const signInWithCode = (code: string) =>
  invoke<void>("sign_in_with_code", { code });

export const signInWithKey = (key: string) =>
  invoke<void>("sign_in_with_key", { key });

export const resolveKeyStorage = (choice: StorageChoice) =>
  invoke<void>("resolve_key_storage", { choice });

export const signOut = () => invoke<void>("sign_out");

export const listModels = (force: boolean) =>
  invoke<ModelInfo[]>("list_models", { force });

export const keyStatus = () => invoke<KeyStatus>("key_status");

export const listChats = () => invoke<ChatSummary[]>("list_chats");

export const getMessages = (chatId: string) =>
  invoke<StoredMessage[]>("get_messages", { chatId });

export const createChat = (modelId: string) =>
  invoke<ChatSummary>("create_chat", { modelId });

export const renameChat = (id: string, title: string) =>
  invoke<void>("rename_chat", { id, title });

export const deleteChat = (id: string) => invoke<void>("delete_chat", { id });

export const chatUsage = (chatId: string) =>
  invoke<ChatUsage>("chat_usage", { chatId });

export const monthSpend = () => invoke<number>("month_spend");

/**
 * Resolves with how the turn ended, once it is stored; rejects if the turn
 * never ran. Reply text arrives through `onEvent`; settle on the promise only.
 */
export function sendMessage(
  chatId: string,
  text: string,
  attachments: AttachmentInput[],
  modelId: string,
  onEvent: (event: EngineEvent) => void,
): Promise<TurnResult> {
  const channel = new Channel<EngineEvent>(onEvent);
  return invoke<TurnResult>("send_message", {
    chatId,
    text,
    attachments,
    modelId,
    onEvent: channel,
  });
}

/**
 * Answers the chat's last stored user message again without storing it twice
 * (an interrupted partial reply is removed first; with `regenerate`, a
 * completed one too). Events as for `sendMessage`.
 */
export function retryTurn(
  chatId: string,
  modelId: string,
  onEvent: (event: EngineEvent) => void,
  regenerate = false,
): Promise<TurnResult> {
  const channel = new Channel<EngineEvent>(onEvent);
  return invoke<TurnResult>("retry_turn", { chatId, modelId, regenerate, onEvent: channel });
}

/** A user message taken back for editing; `removed` counts the rows deleted after it. */
export interface TakenMessage {
  text: string;
  attachments: AttachmentInput[];
  removed: number;
}

/** Deletes the user message and everything after it, returning it for the message box. */
export const takeUserMessage = (chatId: string, messageId: string) =>
  invoke<TakenMessage>("take_user_message", { chatId, messageId });

/** How many rows editing this user message would remove after it. */
export const rowsAfter = (chatId: string, messageId: string) =>
  invoke<number>("rows_after", { chatId, messageId });

/**
 * Summarises the chat in one request on `modelId`; `keep` names what the
 * summary must preserve. Only a completed summary changes the chat.
 */
export const compactChat = (chatId: string, modelId: string, keep: string | null = null) =>
  invoke<TurnResult>("compact_chat", { chatId, modelId, keep });

/** Writes the chat as Markdown to `path`, an absolute `.md` path from a save dialog. */
export const exportChatWithDialog = (chatId: string, suggestedName: string) =>
  invoke<string | null>("export_chat_with_dialog", { chatId, suggestedName });

export const stop = (chatId: string) => invoke<void>("stop", { chatId });

export type PackOrigin = "built_in" | "installed" | "yours";

export interface SkillView {
  id: string;
  name: string;
  description: string;
  /** The skill's own switch, independent of its pack's. */
  on: boolean;
  acts_on_computer: boolean;
  broken: string | null;
  body_chars: number;
  /** On, its pack is on and it is not broken: what the `/` menu offers. */
  available: boolean;
}

export interface PackView {
  id: string;
  title: string;
  origin: PackOrigin;
  license: string;
  repo: string;
  commit: string;
  about: string;
  on: boolean;
  error: string | null;
  skills: SkillView[];
}

export interface SkillDetail {
  skill: SkillView;
  pack_id: string;
  pack_title: string;
  license: string;
  source_url: string;
  body: string;
  files: string[];
}

export interface SkillsStatus {
  /** Set when the bundled skills are missing. */
  builtin_error: string | null;
  packs: number;
  skills: number;
  on: number;
}

export const skillsStatus = () => invoke<SkillsStatus>("skills_status");

export const listPacks = () => invoke<PackView[]>("list_packs");

export const getSkill = (id: string) => invoke<SkillDetail>("get_skill", { id });

export const setSkillOn = (id: string, on: boolean) =>
  invoke<void>("set_skill_on", { id, on });

export const setPackOn = (id: string, on: boolean) =>
  invoke<void>("set_pack_on", { id, on });

/** A file the skill lists, as a text attachment for `sendMessage`. */
export const skillFile = (id: string, file: string) =>
  invoke<AttachmentInput>("skill_file", { id, file });

/** One license file of a pack. */
export interface LicenseFile {
  name: string;
  text: string;
}

/** The pack's license files as text, for the Open-source licenses view. */
export const packLicenses = (id: string) => invoke<LicenseFile[]>("pack_licenses", { id });

/** A pack that is not bundled and can be installed from GitHub. */
export interface ExtraPack {
  id: string;
  title: string;
  about: string;
  license: string;
  repo: string;
  installed: boolean;
  /** Install only after the user accepts a notice (pass `acknowledged`). */
  needs_warning: boolean;
}

/** Where an add comes from. For a folder or a ZIP the core opens the dialog. */
export type AddSource =
  | { kind: "folder" }
  | { kind: "zip" }
  | { kind: "github"; url: string };

/** Skills found and staged by `previewAdd`; confirm or cancel with `token`. */
export interface AddPreview {
  token: string;
  /**
   * `on` is what confirming will do: a skill's stored switch is kept; otherwise
   * an add of more than 10 skills starts them all off, a smaller one on.
   */
  skills: SkillView[];
  /** Files left out, relative to the source. */
  skipped: string[];
}

export const extraPacks = () => invoke<ExtraPack[]>("extra_packs");

/** Downloads the pack at its pinned commit; all its skills start off. */
export const installExtraPack = (id: string, acknowledged: boolean) =>
  invoke<PackView>("install_extra_pack", { id, acknowledged });

/** Removes an installed extra pack; built-in packs and "yours" are refused. */
export const removePack = (id: string) => invoke<void>("remove_pack", { id });

/**
 * Stages the skills found at `source`; `null` when the user cancels the dialog.
 * A GitHub link to a pack already listed in Skills is refused.
 */
export const previewAdd = (source: AddSource) =>
  invoke<AddPreview | null>("preview_add", { source });

/** Adds the previewed skills to "Added by you", replacing same-named ones. */
export const confirmAdd = (token: string) => invoke<PackView>("confirm_add", { token });

export const cancelAdd = (token: string) => invoke<void>("cancel_add", { token });

/** Removes one of the user's own skills; any other id is refused. */
export const removeSkill = (id: string) => invoke<void>("remove_skill", { id });

/**
 * An active skill as it stands for the model asked about. `too_long`: left
 * out of turns on that model. `unavailable`: switched off, broken or removed;
 * kept in the chat but not sent.
 */
export interface ActiveSkill {
  id: string;
  name: string;
  too_long: boolean;
  acts_on_computer: boolean;
  unavailable: boolean;
}

export interface ChatState {
  /** In activation order. */
  active_skills: ActiveSkill[];
  folder: string | null;
  compacted: boolean;
}

/** `percent` is not capped at 100; both are 0 when the model's context length is unknown. */
export interface Fullness {
  percent: number;
  context_length: number;
}

export const chatState = (chatId: string, modelId: string) =>
  invoke<ChatState>("chat_state", { chatId, modelId });

/** Rejects with the exact refusal copy; activating an active skill changes nothing. */
export const activateSkill = (chatId: string, skillId: string, modelId: string) =>
  invoke<ChatState>("activate_skill", { chatId, skillId, modelId });

export const deactivateSkill = (chatId: string, skillId: string) =>
  invoke<ChatState>("deactivate_skill", { chatId, skillId });

/** `draftChars`: characters typed in the message box but not yet sent. */
export const chatFullness = (chatId: string, modelId: string, draftChars: number) =>
  invoke<Fullness>("chat_fullness", { chatId, modelId, draftChars });

export interface FileEntry {
  /** Relative to the chat's folder, with forward slashes. */
  path: string;
  is_dir: boolean;
  bytes: number;
}

/** `total`: entries matching among those walked; `cut`: the walk stopped at 5,000 files. */
export interface FileList {
  entries: FileEntry[];
  total: number;
  cut: boolean;
}

/**
 * The core opens the system folder dialog. Resolves with the chosen folder
 * (now the chat's folder), or `null` when the user cancels.
 */
export const chooseChatFolder = (chatId: string) =>
  invoke<string | null>("choose_chat_folder", { chatId });

/** Folders the user has chosen before, most recent first. */
export const approvedFolders = () => invoke<string[]>("approved_folders");

/**
 * Sets the chat's folder to one the user chose before (see `approvedFolders`);
 * rejects for any other path. `null` clears it.
 */
export const setChatFolder = (chatId: string, path: string | null) =>
  invoke<void>("set_chat_folder", { chatId, path });

/** The first 50 entries of the chat's folder matching `query`; empty when no folder is set. */
export const folderFiles = (chatId: string, query: string) =>
  invoke<FileList>("folder_files", { chatId, query });

/** A file of the chat's folder (or a folder's listing) as an attachment for `sendMessage`. */
export const attachFolderFile = (chatId: string, path: string) =>
  invoke<AttachmentInput>("attach_folder_file", { chatId, path });

/** Opens an OpenRouter page in the browser (allowed only for https://openrouter.ai/*). */
export const openAuthUrl = (url: string) =>
  invoke<void>("plugin:opener|open_url", { url });

/** Opens a GitHub page in the browser; refuses any other address. */
export const openExternal = async (url: string): Promise<void> => {
  if (!url.startsWith("https://github.com/")) throw new Error("That address cannot be opened.");
  await invoke<void>("plugin:opener|open_url", { url });
};

export const onAuthFinished = (
  handler: (result: AuthFinished) => void,
): Promise<UnlistenFn> =>
  listen<AuthFinished>(AUTH_FINISHED, (e) => handler(e.payload));

export const onAuthRevoked = (handler: () => void): Promise<UnlistenFn> =>
  listen<null>(AUTH_REVOKED, () => handler());
