# Beluga Plan 1: Sign-in and Chat Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A cross-platform desktop app where a user signs in with OpenRouter, picks a model, chats with streaming replies, and always sees free requests left and money spent.

**Architecture:** Tauri 2. All network, key handling and storage live in the Rust core; the React interface only calls typed commands and receives events, so the API key never reaches the web view. Chat goes through an `Engine` trait whose first implementation, `DirectEngine`, calls OpenRouter directly with no tools. Plan 2 adds a tool-using engine behind the same trait.

**Tech Stack:** Rust 1.96, Tauri 2, tokio, reqwest (rustls), eventsource-stream, rusqlite (bundled), keyring, wiremock (tests). React 19, TypeScript, Vite, Vitest, Testing Library, pnpm 10. Fonts from `@fontsource` packages.

**Spec:** `docs/superpowers/specs/2026-11-11-beluga-desktop-design.md`. Approved mockup with every colour token: `docs/superpowers/mockups/design-v3.html` (copied in by Task 1).

## Where this plan sits

The spec covers several subsystems that each produce working software, so it is split into five plans. This is Plan 1. The others are written after this one ships, because Plan 2 depends on the outcome of the engine spike.

| Plan | Delivers |
|---|---|
| 1 (this) | Sign-in, key storage, chat, model picker, gauge, appearance modes |
| 2 | goose spike and decision; project folders, file and command tools, approval prompts, "always allow" rules, spend guards |
| 3 | Skills loader, Tier 1 packs, Skills screen, `/` menu |
| 4 | Skill store, Verified manifest, model test harness and "Recommended" marks |
| 5 | Installers and signing for macOS, Windows, Linux; licenses screen |

## Global Constraints

- Repository: `/Users/mdshamirulislam/beluga/beluga-desktop`, its own git repository. The parent folder holds unrelated projects; touch nothing outside the repository except to copy the spec, plan and mockup in.
- License: MIT. `LICENSE` at the root.
- The working name appears in exactly one Rust file (`src-tauri/src/brand.rs`) and one TypeScript file (`src/app/brand.ts`). `tauri.conf.json` `productName` is the only other place.
- The API key never crosses into the web view: no command returns it, no event carries it, it is never logged.
- The only network host the app contacts is `openrouter.ai`. Set the Tauri CSP so the web view can load nothing remote. No telemetry.
- No model id is hard-coded anywhere outside test fixtures.
- Fonts are bundled: Bricolage Grotesque (headings), Hanken Grotesk (body), JetBrains Mono (code).
- No emoji as icons. Icons are inline SVG.
- Colour tokens are copied verbatim from the mockup's `:root` blocks. Amber is used only where the app needs a decision or is warning about the allowance.
- Every task's tests pass on macOS, Windows and Linux in CI before the task is done.
- User-facing copy quoted in this plan is exact.
- Rust: `cargo fmt` and `cargo clippy -- -D warnings` clean. TypeScript: `strict` on.

## Facts to verify before relying on them

The research behind the spec did not confirm these against a live authenticated account. The task that first uses each one checks it against https://openrouter.ai/docs and corrects the fixture and types if it differs.

- `GET /api/v1/key` response field names, including `free_model_daily_requests {used, limit, remaining}` (Task 5).
- Paste-a-code sign-in when `callback_url` is omitted (Task 3).
- `usage: {"include": true}` on a streaming request returns `usage.cost` in the final chunk (Task 6).
- The time of day the free daily limit resets (Task 12). If the API gives no reset time, use 00:00 UTC shown in local time.

## Review Focus

1. The browser never returns from OpenRouter (tab closed, access denied). The sign-in screen must stay usable: Cancel works, the paste option still works, and the listener closes. Tests in Tasks 4 and 9.
2. The network drops mid-reply. The partial reply is kept, marked as interrupted, and Retry is offered. Tests in Tasks 6 and 10.
3. The key is revoked while the app is open (any 401). The app returns to sign-in with an explanation and keeps all chats. Tests in Tasks 8 and 9.
4. The model list cannot be fetched (offline, OpenRouter down). The app still opens using the cached list; with no cache the picker shows a retry. Tests in Tasks 5 and 11.
5. A multi-byte character or an SSE line is split across network chunks. The reply text is not corrupted. Test in Task 6.

---

## File Structure

```
beluga-desktop/
  LICENSE  package.json  vite.config.ts  vitest.setup.ts  index.html
  .github/workflows/ci.yml
  docs/superpowers/{specs,plans,mockups}/
  src/
    main.tsx
    app/        App.tsx  brand.ts  appearance.ts  tokens.css  base.css
    lib/        ipc.ts            typed wrappers for every command and event
    features/
      signin/   SignIn.tsx  KeyStorageChoice.tsx
      shell/    Shell.tsx  Sidebar.tsx  icons.tsx
      chat/     useChat.ts  Thread.tsx  Composer.tsx  EmptyChat.tsx  errors.ts
      models/   ModelPicker.tsx  defaultModel.ts
      gauge/    Gauge.tsx  LowAllowance.tsx
      settings/ Settings.tsx
  src-tauri/
    tauri.conf.json  Cargo.toml  capabilities/default.json
    src/
      main.rs  lib.rs  brand.rs  state.rs  commands.rs
      auth/        mod.rs  pkce.rs  callback.rs
      secrets.rs
      openrouter/  mod.rs  error.rs  key_info.rs  models.rs  stream.rs
      engine/      mod.rs  direct.rs
      store/       mod.rs  chats.rs  usage.rs
    tests/fixtures/  models.json  key_free.json  key_paid.json  stream_*.txt
```

Shared types (defined once, in the task named; later tasks use them as written):

```rust
// openrouter/error.rs (Task 5)
pub enum ApiError { Unauthorized, RateLimited { retry_after_secs: Option<u64> },
    NoCredit, ModelUnavailable, ContextTooLong, Network(String), Other { status: u16, message: String } }

// openrouter/models.rs (Task 5)
pub struct ModelInfo { pub id: String, pub name: String, pub prompt_usd_per_mtok: f64,
    pub completion_usd_per_mtok: f64, pub is_free: bool, pub supports_tools: bool,
    pub accepts_images: bool, pub context_length: u64 }

// openrouter/key_info.rs (Task 5)
pub struct FreeDaily { pub used: u32, pub limit: u32, pub remaining: u32 }
pub struct KeyStatus { pub is_free_tier: bool, pub free_daily: Option<FreeDaily>,
    pub usage_usd: f64, pub limit_remaining_usd: Option<f64> }

// engine/mod.rs (Task 7)
#[non_exhaustive] #[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent { TextDelta { text: String }, Usage { requests: u32, cost_usd: f64 },
    Done, Interrupted { reason: String }, Failed { error: UiError } }
pub struct UiError { pub kind: String, pub message: String, pub retryable: bool }
```

`UiError.kind` is one of `unauthorized`, `rate_limited`, `no_credit`, `model_unavailable`, `context_too_long`, `network`, `other`.

---

### Task 1: Repository scaffold and CI

**Files:**
- Create: everything under `beluga-desktop/` listed as root files above; `src/main.tsx`, `src/app/App.tsx`, `src/app/brand.ts`, `src-tauri/src/{main.rs,lib.rs,brand.rs}`, `.github/workflows/ci.yml`
- Copy in: `../docs/superpowers/specs/2026-11-11-beluga-desktop-design.md`, this plan, and `../.superpowers/brainstorm/71836-1794373193/content/design-v3.html` as `docs/superpowers/mockups/design-v3.html`

**Interfaces:**
- Produces: `brand::APP_NAME: &str = "Beluga"`, `brand::KEYCHAIN_SERVICE: &str = "app.beluga.desktop"`; TS `APP_NAME`. Commands `pnpm test`, `pnpm tauri dev`, `cargo test --manifest-path src-tauri/Cargo.toml`.

- [ ] **Step 1:** `git init` in a new `beluga-desktop/` folder. Scaffold with `pnpm create tauri-app` (React, TypeScript, pnpm), then remove the template's demo command, demo styles and logos.
- [ ] **Step 2:** Add Vitest with jsdom and Testing Library (`vitest.setup.ts` imports `@testing-library/jest-dom/vitest`). Add script `"test": "vitest run"`.
- [ ] **Step 3: Write the failing tests.** `src/app/App.test.tsx`: renders `<App />` and expects a heading with text `APP_NAME`. `src-tauri/src/brand.rs`: a unit test asserting `APP_NAME == "Beluga"` and `KEYCHAIN_SERVICE` contains no spaces.
- [ ] **Step 4:** Run `pnpm test` and `cargo test --manifest-path src-tauri/Cargo.toml`. Expected: both fail (missing `brand`).
- [ ] **Step 5:** Implement `brand.ts`, `brand.rs`, and a minimal `App.tsx`. Set `productName` to `Beluga`, identifier `app.beluga.desktop`, and a CSP of `default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:` in `tauri.conf.json`.
- [ ] **Step 6:** Run both test commands. Expected: pass. Run `pnpm tauri dev`. Expected: a window titled Beluga opens.
- [ ] **Step 7:** Add `LICENSE` (MIT, copyright "Beluga contributors") and `ci.yml`: matrix `macos-latest`, `windows-latest`, `ubuntu-latest`; install Linux Tauri system packages on Ubuntu; run `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `pnpm install --frozen-lockfile`, `pnpm exec tsc --noEmit`, `pnpm test`.
- [ ] **Step 8:** Copy the spec, plan and mockup in. Commit: `chore: scaffold Tauri app with tests and CI`.

### Task 2: Colour tokens and appearance modes

**Files:**
- Create: `src/app/tokens.css`, `src/app/base.css`, `src/app/appearance.ts`, `src/app/appearance.test.ts`
- Modify: `src/main.tsx`, `index.html`, `package.json` (add `@fontsource-variable/bricolage-grotesque`, `@fontsource-variable/hanken-grotesk`, `@fontsource/jetbrains-mono`)

**Interfaces:**
- Produces: `type Appearance = "system" | "light" | "dark"`; `getAppearance(): Appearance`; `setAppearance(a: Appearance): void`; `applyAppearance(a: Appearance): void`. CSS custom properties named exactly as in the mockup (`--ice`, `--floe`, `--line`, `--deep`, `--mid`, `--sonar`, `--amber-soft`, `--white`, `--sel`, `--tick-used`, `--ask-line`, `--ask-ink`, `--field-line`, `--ph`, `--btn`, `--on-btn`, `--on-accent`, `--pitch`, `--shadow`, `--frame`, `--text2`). Drop `--page` and `--dot`, which only styled the mockup page.

- [ ] **Step 1: Write the failing tests** in `appearance.test.ts`:

```ts
it("defaults to system", () => { localStorage.clear(); expect(getAppearance()).toBe("system"); });
it("system removes the attribute", () => { applyAppearance("system");
  expect(document.documentElement.hasAttribute("data-theme")).toBe(false); });
it("dark sets the attribute and persists", () => { setAppearance("dark");
  expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  expect(localStorage.getItem("appearance")).toBe("dark"); });
it("ignores a corrupt stored value", () => { localStorage.setItem("appearance", "neon");
  expect(getAppearance()).toBe("system"); });
```

- [ ] **Step 2:** Run `pnpm test appearance`. Expected: fail, module not found.
- [ ] **Step 3:** Implement `appearance.ts`. Copy the three token blocks from the mockup into `tokens.css` unchanged in structure: `:root`, `@media (prefers-color-scheme:dark){:root:not([data-theme="light"])}`, `:root[data-theme="dark"]`. `base.css` sets body font, background `var(--ice)`, colour `var(--deep)`, and a visible `:focus-visible` outline in `var(--sonar)`.
- [ ] **Step 4:** Add an inline script in `index.html` `<head>` that applies the stored appearance before first paint, so a dark-mode user never sees a light flash.
- [ ] **Step 5:** Run `pnpm test`. Expected: pass. Commit: `feat: colour tokens and system/light/dark appearance`.

### Task 3: PKCE and code exchange

**Files:**
- Create: `src-tauri/src/auth/mod.rs`, `src-tauri/src/auth/pkce.rs`
- Modify: `src-tauri/Cargo.toml` (`sha2`, `base64`, `rand`, `reqwest` with `json`, `rustls-tls`, `stream`; dev `wiremock`, `tokio` test features)

**Interfaces:**
- Produces:
  - `pub struct Pkce { pub verifier: String, pub challenge: String }`
  - `pub fn generate() -> Pkce` (verifier: 64 random URL-safe characters)
  - `pub fn challenge_for(verifier: &str) -> String`
  - `pub fn auth_url(challenge: &str, callback_url: Option<&str>, state: &str) -> String`
  - `pub async fn exchange_code(http: &reqwest::Client, base_url: &str, code: &str, verifier: &str) -> Result<String, ApiError>` (returns the key)
  - `base_url` is `https://openrouter.ai` in production and the mock server in tests. Every OpenRouter function in this plan takes `base_url` the same way.

- [ ] **Step 1: Write the failing tests.**

```rust
#[test] fn rfc7636_vector() {
    assert_eq!(challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
               "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"); }
#[test] fn verifier_is_64_urlsafe_chars_and_unique() { /* len 64, [A-Za-z0-9_-], two calls differ */ }
#[test] fn auth_url_with_callback() {
    let u = auth_url("CH", Some("http://localhost:5123/callback"), "ST");
    assert!(u.starts_with("https://openrouter.ai/auth?"));
    assert!(u.contains("callback_url=http%3A%2F%2Flocalhost%3A5123%2Fcallback"));
    assert!(u.contains("code_challenge=CH") && u.contains("code_challenge_method=S256") && u.contains("state=ST")); }
#[test] fn auth_url_without_callback_omits_it() { assert!(!auth_url("CH", None, "ST").contains("callback_url")); }
#[tokio::test] async fn exchange_posts_code_and_returns_key() {
    // wiremock: POST /api/v1/auth/keys, body_json {"code":"c1","code_verifier":"v1","code_challenge_method":"S256"} -> 200 {"key":"sk-or-test"}
}
#[tokio::test] async fn exchange_maps_403_to_other_and_never_panics_on_html_body() {}
```

- [ ] **Step 2:** Run `cargo test auth::pkce`. Expected: fail to compile. `ApiError` does not exist yet: define the enum from the Shared types block in a new `openrouter/error.rs` now, with no mapping logic (Task 5 adds it).
- [ ] **Step 3:** Implement the four functions.
- [ ] **Step 4:** Check the OpenRouter OAuth PKCE documentation for the paste-a-code mode. If omitting `callback_url` is not supported, delete `auth_url`'s `None` branch and its test, and note in the commit message that the paste path in Task 9 is "paste a key" only.
- [ ] **Step 5:** Run `cargo test auth::pkce`. Expected: pass. Commit: `feat: OpenRouter PKCE challenge and code exchange`.

### Task 4: Loopback callback listener

**Files:**
- Create: `src-tauri/src/auth/callback.rs`

**Interfaces:**
- Produces:
  - `pub struct Listener { pub callback_url: String /* http://localhost:{port}/callback */, .. }`
  - `pub async fn bind() -> std::io::Result<Listener>` (binds `127.0.0.1:0`)
  - `pub async fn wait_for_code(self, expected_state: &str, timeout: Duration, cancel: CancellationToken) -> Result<String, CallbackError>`
  - `pub enum CallbackError { TimedOut, Cancelled, Denied, Io(String) }`
  - Production timeout: 10 minutes.

- [ ] **Step 1: Write the failing tests** (each binds a real listener and calls it with `reqwest`):

```rust
// returns_code_when_state_matches: GET {url}?code=abc&state=ST -> Ok("abc"); response body contains "You can close this tab"
// ignores_wrong_state_and_keeps_waiting: GET ?code=x&state=BAD gets 400; a following good request still returns Ok
// ignores_other_paths: GET /favicon.ico gets 404 and does not end the wait
// error_param_is_denied: GET /callback?error=access_denied&state=ST -> Err(Denied)
// times_out: timeout 50ms, no request -> Err(TimedOut)
// cancel_ends_wait_and_frees_port: cancel -> Err(Cancelled); a new TcpListener can bind the same port afterwards
// survives_garbage_bytes: raw TCP write of b"\xff\xfe\r\n\r\n" does not end the wait or panic
```

- [ ] **Step 2:** Run `cargo test auth::callback`. Expected: fail to compile.
- [ ] **Step 3:** Implement with `tokio::net::TcpListener`, reading only the request line and parsing the query with the `url` crate. No web framework. The success page is one static HTML string using `brand::APP_NAME`.
- [ ] **Step 4:** Run `cargo test auth::callback`. Expected: pass on all three CI systems. Commit: `feat: loopback listener for the sign-in callback`.

### Task 5: Key storage and OpenRouter read endpoints

**Files:**
- Create: `src-tauri/src/secrets.rs`, `src-tauri/src/openrouter/{mod.rs,key_info.rs,models.rs}`, `src-tauri/tests/fixtures/{models.json,key_free.json,key_paid.json}`
- Modify: `src-tauri/src/openrouter/error.rs`

**Interfaces:**
- Produces:
  - `pub enum StorageKind { Keychain, Session, File }`
  - `pub trait KeyStore: Send + Sync { fn get(&self) -> Result<Option<String>, SecretError>; fn set(&self, key: &str) -> Result<(), SecretError>; fn clear(&self) -> Result<(), SecretError>; fn kind(&self) -> StorageKind; }`
  - `pub enum SecretError { Unavailable, Other(String) }` (`Unavailable` means no keychain service on this machine)
  - `KeychainStore::new()`, `SessionStore::default()`, `FileStore::new(path: PathBuf)`
  - `impl ApiError { pub fn from_response(status: u16, body: &str) -> ApiError }`
  - `pub async fn key_status(http, base_url, key) -> Result<KeyStatus, ApiError>`
  - `pub async fn list_models(http, base_url) -> Result<Vec<ModelInfo>, ApiError>`
  - `pub struct ModelCache { .. }` with `load(path) -> Option<Vec<ModelInfo>>`, `save(path, &[ModelInfo])`

- [ ] **Step 1: Capture fixtures.** `curl -s https://openrouter.ai/api/v1/models` and keep six entries in `models.json`: two `:free` with tools, one `:free` without tools, two paid with tools (one accepting images), one paid without tools. Write `key_free.json` and `key_paid.json` from the documented shape of `GET /api/v1/key`, and confirm the field names against the docs.
- [ ] **Step 2: Write the failing tests.**

```rust
// secrets
// session_store_roundtrip: set, get == Some, clear, get == None; kind() == Session
// file_store_is_user_only (cfg(unix)): after set, file mode & 0o777 == 0o600
// file_store_roundtrip_and_clear_removes_file
// models
// parses_fixture: 6 models; ":free" ids have is_free == true and both prices 0.0
// price_is_per_million: a fixture model priced "0.0000005" per token -> prompt_usd_per_mtok == 0.5
// supports_tools_from_supported_parameters; accepts_images_from_architecture_input_modalities
// skips_an_entry_with_missing_pricing_instead_of_failing_the_whole_list
// cache_roundtrip; cache_load_returns_none_on_corrupt_file
// key_info
// free_tier_fixture: is_free_tier true, free_daily == Some(FreeDaily{used:12,limit:50,remaining:38})
// paid_fixture: limit_remaining_usd == Some(4.2), usage_usd == 0.8
// errors
// from_response(401, _) == Unauthorized; 402 -> NoCredit; 429 -> RateLimited;
// 404 or 503 with "No endpoints found" -> ModelUnavailable;
// 400 with "context length" in the message -> ContextTooLong; an HTML body -> Other with a trimmed message
```

- [ ] **Step 3:** Run `cargo test secrets openrouter`. Expected: fail to compile.
- [ ] **Step 4:** Implement. `KeychainStore` uses the `keyring` crate with service `brand::KEYCHAIN_SERVICE`, account `openrouter`, and maps a missing platform backend to `SecretError::Unavailable`. No test touches the real keychain. `ModelCache` writes JSON to the app data folder.
- [ ] **Step 5:** Run the tests. Expected: pass. Commit: `feat: key storage and OpenRouter key and model endpoints`.

### Task 6: Streaming chat completions

**Files:**
- Create: `src-tauri/src/openrouter/stream.rs`, `src-tauri/tests/fixtures/stream_{ok,split_utf8,error_mid,no_done}.txt`

**Interfaces:**
- Consumes: `ApiError`.
- Produces:
  - `pub enum Part { Text(String), ImageDataUrl(String) }`
  - `pub struct WireMessage { pub role: Role, pub parts: Vec<Part> }`, `pub enum Role { User, Assistant, System }`
  - `pub enum Delta { Text(String), Usage { cost_usd: f64, prompt_tokens: u64, completion_tokens: u64 }, Done }`
  - `pub async fn chat_stream(http, base_url, key, model_id: &str, messages: &[WireMessage]) -> Result<impl Stream<Item = Result<Delta, ApiError>>, ApiError>`
  - Request: `POST /api/v1/chat/completions`, `stream: true`, `usage: {"include": true}`, header `X-Title: brand::APP_NAME`. No `HTTP-Referer` until the project has a public home page.

- [ ] **Step 1: Write the failing tests** against wiremock responses built from the fixtures:

```rust
// ok: deltas "Hel","lo" then a usage chunk then [DONE] -> [Text("Hel"), Text("lo"), Usage{cost_usd:0.0,..}, Done]
// comment_lines_are_ignored: ": OPENROUTER PROCESSING" lines produce no Delta
// split_utf8: the bytes of "নমস্কার" split mid-character across two body chunks -> concatenated text equals "নমস্কার"
// error_mid: a data chunk carrying {"error":{"code":429,..}} after one Text -> Text, then Err(RateLimited{..})
// no_done: body ends without [DONE] -> Text items then Err(Network(_))
// http_401_before_stream -> Err(Unauthorized) from chat_stream itself
// request_shape: body has stream true, usage.include true, and an image part serialises as {"type":"image_url","image_url":{"url":"data:..."}}
// key_is_sent_as_bearer_and_not_in_the_url
```

- [ ] **Step 2:** Run `cargo test openrouter::stream`. Expected: fail to compile.
- [ ] **Step 3:** Implement using `eventsource-stream` over `reqwest`'s byte stream. Confirm against the docs that `usage.cost` arrives in the final chunk; if the field differs, fix the fixture and parser together.
- [ ] **Step 4:** Run the tests. Expected: pass. Commit: `feat: streaming chat completions client`.

### Task 7: Engine boundary and DirectEngine

**Files:**
- Create: `src-tauri/src/engine/{mod.rs,direct.rs}`

**Interfaces:**
- Consumes: `chat_stream`, `Delta`, `WireMessage`, `ApiError`.
- Produces:
  - `EngineEvent`, `UiError` as in the Shared types block; `impl From<ApiError> for UiError`
  - `pub struct TurnRequest { pub model_id: String, pub messages: Vec<WireMessage> }`
  - `pub struct TurnOutcome { pub text: String, pub requests: u32, pub cost_usd: f64, pub interrupted: bool }`
  - `#[async_trait] pub trait Engine: Send + Sync { async fn run_turn(&self, req: TurnRequest, emit: &(dyn Fn(EngineEvent) + Send + Sync), cancel: CancellationToken) -> TurnOutcome; }`
  - `DirectEngine::new(http, base_url, key_source: Arc<dyn KeyStore>)`
  - `UiError.message` copy, exact:
    - `unauthorized`: "OpenRouter no longer accepts this key. Sign in again to continue."
    - `rate_limited`: "OpenRouter is limiting requests right now. Wait a moment, then try again."
    - `no_credit`: "This model needs credit and your OpenRouter balance is empty. Pick a free model or add credit."
    - `model_unavailable`: "This model is not available right now. Pick another model and try again."
    - `context_too_long`: "This chat is too long for the model. Start a new chat or pick a model with a larger limit."
    - `network`: "Could not reach OpenRouter. Check your connection and try again."
    - `other`: "OpenRouter returned an error: {message}"
  - `retryable` is true for `rate_limited`, `model_unavailable`, `network`.

- [ ] **Step 1: Write the failing tests** using a wiremock server:

```rust
// emits_deltas_then_usage_then_done: events == [TextDelta "Hel", TextDelta "lo", Usage{requests:1,cost_usd:0.0}, Done]; outcome.text == "Hello"
// failure_before_any_text_emits_failed_with_exact_copy (one case per kind above)
// failure_after_text_emits_interrupted_and_keeps_partial: outcome.text == "Hel", outcome.interrupted == true, requests == 1
// cancel_stops_promptly: cancel after first delta -> Interrupted{reason:"stopped"}, outcome.text holds what arrived
// missing_key_emits_failed_unauthorized_without_any_http_call
// event_json_shape: serde_json of TextDelta{text:"a"} == {"type":"text_delta","text":"a"}
```

- [ ] **Step 2:** Run `cargo test engine`. Expected: fail to compile.
- [ ] **Step 3:** Implement. `DirectEngine` reads the key from the store at the start of each turn, so sign-out takes effect immediately.
- [ ] **Step 4:** Run the tests. Expected: pass. Commit: `feat: engine boundary with direct OpenRouter engine`.

### Task 8: Local store and Tauri commands

**Files:**
- Create: `src-tauri/src/store/{mod.rs,chats.rs,usage.rs}`, `src-tauri/src/state.rs`, `src-tauri/src/commands.rs`, `src/lib/ipc.ts`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/capabilities/default.json`, `src-tauri/Cargo.toml` (`rusqlite` with `bundled`, `tauri-plugin-opener`, `uuid`, `time`)

**Interfaces:**
- Consumes: everything from Tasks 3 to 7.
- Produces (Rust store):
  - `Store::open(path) -> Result<Store>`, `Store::in_memory()`
  - `ChatSummary { id: String, title: String, model_id: String, updated_at: i64 }`
  - `StoredMessage { id: String, role: String, text: String, attachments: Vec<StoredAttachment>, interrupted: bool, created_at: i64 }`
  - `create_chat(model_id) -> ChatSummary`, `list_chats()` (newest first), `get_messages(chat_id)`, `append_message(..)`, `rename_chat(id, title)`, `delete_chat(id)`
  - `record_usage(chat_id, requests: u32, cost_usd: f64)`, `chat_usage(chat_id) -> (u32, f64)`, `month_spend(now) -> f64`
- Produces (commands, each mirrored by a typed function of the same name in camelCase in `ipc.ts`):

| Command | Arguments | Returns |
|---|---|---|
| `auth_status` | | `{ signed_in: bool, storage: "keychain" \| "session" \| "file" \| null }` |
| `sign_in_start` | | `{ auth_url: String }`; later emits event `auth://finished` with `{ ok: bool, reason: "denied" \| "timed_out" \| "cancelled" \| "keychain_unavailable" \| "failed" \| null }` |
| `sign_in_cancel` | | `()` |
| `sign_in_with_code` | `code` | `Result<(), UiError>` |
| `sign_in_with_key` | `key` | `Result<(), UiError>` (validates with `key_status` before saving) |
| `resolve_key_storage` | `choice: "session" \| "file"` | `Result<(), UiError>` (saves the key held after a `keychain_unavailable` result) |
| `sign_out` | | `()` |
| `list_models` | `force: bool` | `Result<Vec<ModelInfo>, UiError>` (cache for 10 minutes; on failure return the cached list if any) |
| `key_status` | | `Result<KeyStatus, UiError>` |
| `list_chats`, `get_messages`, `create_chat`, `rename_chat`, `delete_chat` | as the store | as the store |
| `chat_usage` | `chat_id` | `{ requests: u32, cost_usd: f64 }` |
| `month_spend` | | `f64` |
| `send_message` | `chat_id, text, attachments: Vec<AttachmentInput>, model_id, on_event: Channel<EngineEvent>` | `()` |
| `stop` | `chat_id` | `()` |

  - `AttachmentInput { name: String, mime: String, data_base64: String }`
  - On any `unauthorized` error from any command, the core clears nothing, emits event `auth://revoked`, and reports `signed_in: false` from then on.

- [ ] **Step 1: Write the failing store tests.**

```rust
// create_then_list_newest_first; rename; delete_removes_messages_and_usage
// title_defaults_to_first_40_chars_of_first_user_message_on_a_word_boundary
// month_spend_sums_only_the_current_calendar_month
// reopening_the_same_file_keeps_chats (tempfile)
// a_message_with_interrupted_true_roundtrips
```

- [ ] **Step 2: Write the failing command tests.** Command bodies are plain `async fn`s taking `&AppState`, so tests build an `AppState` with `Store::in_memory()`, `SessionStore`, and a wiremock `base_url`.

```rust
// sign_in_with_key_rejects_a_401_key_and_saves_nothing
// sign_in_with_key_saves_then_auth_status_is_signed_in
// keychain_unavailable_holds_the_key_until_resolve_key_storage("session")
// sign_out_clears_the_key_and_keeps_chats
// send_message_persists_user_and_assistant_messages_and_records_usage
// send_message_interrupted_persists_partial_text_with_interrupted_true
// send_message_401_emits_auth_revoked_and_auth_status_becomes_signed_out; list_chats still returns the chats
// list_models_falls_back_to_cache_when_the_server_is_down
// stop_cancels_the_running_turn_for_that_chat_only
// no_command_return_value_or_event_payload_contains_the_key (serialise each and assert the key string is absent)
```

- [ ] **Step 3:** Run `cargo test store commands`. Expected: fail to compile.
- [ ] **Step 4:** Implement store, `AppState` (`http`, `base_url`, `Arc<dyn KeyStore>`, `Store`, `Arc<dyn Engine>`, running-turn map, pending sign-in), commands, and registration in `lib.rs`. `sign_in_start` binds the listener, opens the browser with `tauri-plugin-opener`, and spawns the wait. Choose `KeychainStore` at startup and fall back per `resolve_key_storage`. The database lives in the Tauri app data directory.
- [ ] **Step 5:** Write `ipc.ts`: one exported function per command and `onAuthFinished`, `onAuthRevoked` listeners, with TypeScript types matching the Rust structs field for field.
- [ ] **Step 6:** Run `cargo test` and `pnpm exec tsc --noEmit`. Expected: pass. Commit: `feat: local chat store and typed command layer`.

### Task 9: Sign-in screen

**Files:**
- Create: `src/features/signin/{SignIn.tsx,SignIn.test.tsx,KeyStorageChoice.tsx}`, `src/features/shell/icons.tsx`
- Modify: `src/app/App.tsx`

**Interfaces:**
- Consumes: `authStatus`, `signInStart`, `signInCancel`, `signInWithCode`, `signInWithKey`, `resolveKeyStorage`, `onAuthFinished`, `onAuthRevoked` from `ipc.ts`.
- Produces: `<SignIn notice?: string onSignedIn={() => void} />`. `App` shows `SignIn` when signed out and the shell (Task 10) when signed in.

Exact copy:
- Brand panel heading: "A senior developer's habits, on a free model."
- Brand panel body: "Plans before it codes, tests what it writes, and asks before it touches your machine."
- Form heading: "Connect your OpenRouter account"
- Form body: "Beluga has no subscription. It runs on your own OpenRouter account, which is free to open." (name from `APP_NAME`)
- Primary button: "Sign in with OpenRouter". Secondary: "Paste a key I already have".
- Footnote: "Your key stays in this computer's keychain and is never sent to us."
- Waiting state: "Finish signing in in your browser." with a "Cancel" button.
- After `auth://revoked`: notice "OpenRouter no longer accepts this key. Sign in again to continue. Your chats are still here."
- Free-tier note under the buttons: "Free models allow 50 requests a day. A one-time $10 top-up on OpenRouter raises that to 1,000 a day."
- Keychain unavailable dialog: heading "This computer has no keychain service", body "Beluga can keep your key until you quit, or save it in a file only your account can read. A file is less protected than a keychain.", buttons "Keep until I quit" (default) and "Save in a file".

- [ ] **Step 1: Write the failing tests** with `ipc.ts` mocked:

```ts
// shows both buttons and the exact headings
// clicking "Sign in with OpenRouter" calls signInStart and shows the waiting state
// Cancel calls signInCancel and returns to the buttons
// auth finished {ok:false, reason:"denied"} shows "OpenRouter sign-in was declined. Try again, or paste a key." and both buttons work again
// auth finished {ok:false, reason:"timed_out"} shows "Sign-in took too long and was cancelled. Try again."
// "Paste a key I already have" reveals a password-type field; submitting calls signInWithKey; a rejected key shows the UiError message and keeps the field
// reason "keychain_unavailable" opens the dialog; "Keep until I quit" calls resolveKeyStorage("session")
// the pasted key is cleared from component state after submit
// the brand panel keeps its navy background when data-theme="light" and "dark" (class uses var(--pitch))
```

- [ ] **Step 2:** Run `pnpm test SignIn`. Expected: fail.
- [ ] **Step 3:** Implement to match the mockup's "First open" screen: two columns, brand panel left, form right, single column under 760px.
- [ ] **Step 4:** Run `pnpm test`. Expected: pass. Run `pnpm tauri dev` and sign in with a real OpenRouter account. Expected: browser opens, authorising returns to the app, the app shows the signed-in state. Commit: `feat: sign-in screen with OpenRouter and paste-a-key paths`.

### Task 10: Shell, conversation and message box

**Files:**
- Create: `src/features/shell/{Shell.tsx,Sidebar.tsx}`, `src/features/chat/{useChat.ts,Thread.tsx,Composer.tsx,EmptyChat.tsx,errors.ts}` and their `.test.tsx` files
- Modify: `src/app/App.tsx`

**Interfaces:**
- Consumes: chat commands, `sendMessage`, `stop`, `EngineEvent`.
- Produces:
  - `useChat(chatId: string | null)` returning `{ messages, streamingText, status: "idle" | "streaming" | "failed" | "interrupted", error: UiError | null, send(text, attachments), stop(), retry() }`
  - `<Composer onSend disabled slots={{ model: ReactNode }} />` (Task 11 fills the `model` slot)
  - `<Sidebar footer={ReactNode} />` (Task 12 fills `footer`)
  - `<Shell />` with a header exposing a `usage` slot (Task 12)

Exact copy: greeting "What are we building?"; sub-line "Ask a question, paste an error, or think a problem through." (project wording arrives with Plan 2); placeholder "Describe a feature, paste an error, or ask a question"; in a chat with messages, placeholder "Reply"; sidebar button "New chat"; group label "Earlier"; interrupted marker "Reply interrupted." with button "Retry"; stop button label "Stop"; delete confirmation "Delete this chat? This cannot be undone." with "Delete chat" and "Keep chat".

- [ ] **Step 1: Write the failing tests.**

```ts
// useChat: text_delta events accumulate into streamingText; done moves it into messages and status "idle"
// useChat: failed sets error and status "failed", and the user's message stays in the list
// useChat: interrupted keeps the partial assistant message with interrupted true; retry() resends the last user message
// useChat: events for a chat that is no longer selected do not change the visible thread
// Composer: Enter sends, Shift+Enter inserts a newline, empty or whitespace-only text does not send, disabled while streaming
// Composer: while streaming the send control becomes "Stop" and calls stop()
// Thread: the view follows new text unless the user has scrolled up
// Thread: assistant text renders markdown code blocks in JetBrains Mono and never renders raw HTML from the model
// Sidebar: lists chats newest first; selecting one loads it; rename edits in place; delete asks for confirmation
// EmptyChat: shows the greeting; sending the first message creates the chat and selects it
// errors.ts: each UiError.kind maps to a next step (unauthorized -> sign-in, no_credit -> open the model picker, retryable -> Retry button)
```

- [ ] **Step 2:** Run `pnpm test`. Expected: fail.
- [ ] **Step 3:** Implement to match the mockup's "New chat" and "Mid-task" screens: 232px sidebar, conversation column max 680px, the user's messages right-aligned on `var(--sel)`, assistant text unboxed. Render markdown with `react-markdown` (no `rehype-raw`). Three starter actions from the mockup are left out of this plan; they arrive with skills in Plan 3.
- [ ] **Step 4:** Run `pnpm test`. Expected: pass. In `pnpm tauri dev`, hold a streamed conversation, stop one reply mid-way, turn off the network mid-reply and confirm "Reply interrupted." with Retry. Commit: `feat: chat shell with streaming conversation`.

### Task 11: Model picker

**Files:**
- Create: `src/features/models/{ModelPicker.tsx,defaultModel.ts}` and tests
- Modify: `src/features/chat/Composer.tsx` (pass the slot), `src/features/shell/Shell.tsx`

**Interfaces:**
- Consumes: `listModels`, `ModelInfo`.
- Produces: `pickDefaultModel(models: ModelInfo[]): ModelInfo | null`; `<ModelPicker value onChange />`; the chosen model id is remembered per chat (stored on the chat) and the last choice is the default for new chats (`localStorage` key `lastModel`).

Default rule for this plan (Plan 4 replaces it with "Recommended" marks): among models with `is_free && supports_tools`, the one with the largest `context_length`; ties broken by id ascending. If none, the largest-context free model. If no free models, `null` and the picker opens.

- [ ] **Step 1: Write the failing tests.**

```ts
// pickDefaultModel: picks the free tool-capable model with the largest context; tie -> lowest id; no tool-capable free -> largest free; none free -> null
// a remembered lastModel that is no longer in the list falls back to pickDefaultModel
// picker groups under "Free" and "Paid"; paid rows show "$0.50 in, $1.50 out per million tokens"; free rows show "Free"
// typing filters by name and id
// list failure with no cache shows "Could not load models." and a "Try again" button that calls listModels(true)
// the chip in the message box shows the model name and "Free" in var(--sonar) for a free model
```

- [ ] **Step 2:** Run `pnpm test models`. Expected: fail.
- [ ] **Step 3:** Implement. Keyboard: arrow keys move, Enter selects, Escape closes.
- [ ] **Step 4:** Run `pnpm test`. Expected: pass. Commit: `feat: live model picker with free and paid groups`.

### Task 12: Free-request gauge, cost and running-low behaviour

**Files:**
- Create: `src/features/gauge/{Gauge.tsx,LowAllowance.tsx}` and tests
- Modify: `src/features/shell/{Shell.tsx,Sidebar.tsx}`, `src/features/chat/useChat.ts` (refresh after each turn)

**Interfaces:**
- Consumes: `keyStatus`, `monthSpend`, `chatUsage`, `listModels`.
- Produces: `<Gauge status={KeyStatus} monthSpend={number} usingFreeModel={boolean} />`; `<LowAllowance models={ModelInfo[]} onPick />`.

Behaviour and exact copy:
- Free model selected: "{remaining} of {limit} free requests left", a grid of marks, "Resets at {local time}.", "Spent this month: ${x.xx}".
- Marks: one per request when `limit <= 50`; when `limit` is larger, 50 marks each standing for `limit / 50` requests, a mark counting as spent once all its requests are used.
- `remaining <= 10`: the number and remaining marks use the amber text colour `var(--ask-ink)`.
- `remaining == 0`: sending on a free model is blocked and `LowAllowance` shows "You have used today's free requests. They reset at {local time}." followed by the three cheapest paid tool-capable models with prices and the line "A one-time $10 top-up on OpenRouter raises the free limit to 1,000 requests a day." The app never changes the model itself.
- Paid model selected: marks are replaced by "Spent this month: ${x.xx}" and "Balance: ${y.yy}" (or "Balance: no limit set" when `limit_remaining_usd` is null).
- Header: "This chat: {n} requests, ${x.xx}".
- The gauge refreshes after every turn and when the window regains focus.

- [ ] **Step 1: Write the failing tests.**

```ts
// 38 of 50: 12 spent marks, 38 remaining marks, text "38 of 50 free requests left"
// 950 of 1000: 50 marks, 2 spent (50 used / 20 per mark, rounded down)
// remaining 10 uses the amber class; remaining 11 does not
// remaining 0 blocks send on a free model and renders LowAllowance with exactly three paid tool-capable models, cheapest first
// remaining 0 with a paid model selected does not block send
// paid model: no marks, shows Balance
// free_daily null on a free-tier key: shows "Free requests left: unknown" and never blocks
// keyStatus failure keeps the last known numbers and shows "Could not refresh."
// header shows "This chat: 7 requests, $0.00"
```

- [ ] **Step 2:** Run `pnpm test gauge`. Expected: fail.
- [ ] **Step 3:** Confirm the daily reset time in the OpenRouter limits documentation, then implement.
- [ ] **Step 4:** Run `pnpm test`. Expected: pass. Commit: `feat: free-request gauge and cost display`.

### Task 13: Attachments and Settings

**Files:**
- Create: `src/features/settings/Settings.tsx` and test, `src/features/chat/attachments.ts` and test
- Modify: `src/features/chat/Composer.tsx`, `src-tauri/src/commands.rs` (build `WireMessage` parts from `AttachmentInput`), `src/features/shell/Sidebar.tsx`

**Interfaces:**
- Consumes: `AttachmentInput`, `ModelInfo.accepts_images`, `setAppearance`, `signOut`, `authStatus`.
- Produces: `classifyFile(file: File): "text" | "image" | "unsupported"`; limits `MAX_TEXT_BYTES = 200_000`, `MAX_IMAGE_BYTES = 5_000_000`, `MAX_ATTACHMENTS = 5`.

Behaviour and exact copy:
- Text files are sent inline as a text part: a line `File: {name}` followed by the contents in a fenced block.
- Images are sent as image parts only when the selected model accepts images; otherwise the attachment is refused with "This model cannot read images. Pick a model that can, or remove the image."
- Oversize: "{name} is too large to attach." Unsupported type: "{name} is not a text file or image."
- Settings: an Appearance control with "System", "Light", "Dark"; "Key storage: {This computer's keychain | Until you quit | A file on this computer}"; a "Sign out" button with confirmation "Sign out? Your chats stay on this computer."

- [ ] **Step 1: Write the failing tests.**

```ts
// classifyFile: .ts, .md, .json and an extensionless UTF-8 file -> "text"; .png, .jpg -> "image"; .zip and a binary blob -> "unsupported"
// a 300 KB text file is refused with the exact oversize copy
// a sixth attachment is refused
// an image with a model where accepts_images is false shows the exact refusal and is not sent
// Settings: choosing Dark calls setAppearance("dark"); the current choice is marked pressed
// Settings: Sign out asks first; confirming calls signOut and returns to SignIn
```
```rust
// commands: a text AttachmentInput becomes a Text part beginning "File: notes.md"
// commands: an image AttachmentInput becomes ImageDataUrl("data:image/png;base64,...")
// commands: an image sent to a model whose cached ModelInfo has accepts_images false is rejected with UiError kind "other"
```

- [ ] **Step 2:** Run `pnpm test` and `cargo test commands`. Expected: fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run all tests. Expected: pass. In `pnpm tauri dev` on each of the three systems available, walk through: sign in, chat, attach a file, switch all three appearance modes, sign out. Commit: `feat: attachments and settings`.
