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

/** Resolves when the turn has ended; progress arrives through `onEvent`. */
export function sendMessage(
  chatId: string,
  text: string,
  attachments: AttachmentInput[],
  modelId: string,
  onEvent: (event: EngineEvent) => void,
): Promise<void> {
  const channel = new Channel<EngineEvent>(onEvent);
  return invoke<void>("send_message", {
    chatId,
    text,
    attachments,
    modelId,
    onEvent: channel,
  });
}

export const stop = (chatId: string) => invoke<void>("stop", { chatId });

/** Opens an OpenRouter page in the browser (allowed only for https://openrouter.ai/*). */
export const openAuthUrl = (url: string) =>
  invoke<void>("plugin:opener|open_url", { url });

export const onAuthFinished = (
  handler: (result: AuthFinished) => void,
): Promise<UnlistenFn> =>
  listen<AuthFinished>(AUTH_FINISHED, (e) => handler(e.payload));

export const onAuthRevoked = (handler: () => void): Promise<UnlistenFn> =>
  listen<null>(AUTH_REVOKED, () => handler());
