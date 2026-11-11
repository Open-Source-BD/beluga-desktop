# Plan 1 follow-ups

Carried out of the Plan 1 build (branch plan-1-chat-foundation). Source: the build ledger.

## Not yet verified

- The app has never been run with a window, and no real OpenRouter sign-in has been made.
- CI has never run: nothing has been compiled or tested on Windows or Linux.
- Whether OpenRouter accepts the http://localhost:<port>/callback redirect and the no-callback paste-a-code flow in practice (docs say yes).
- Whether the CSP blocks Vite's dev-mode preamble under `pnpm tauri dev`.
- Free daily limit reset time: shown as 00:00 UTC (docs say "current UTC day", no explicit reset time).

## Owner decision

- Deleting a chat lowers "Spent this month" (usage rows cascade). Recommended: keep usage rows.

## Rulings made during the build

- Ruling: R0 no git worktree; work on branch plan-1-chat-foundation in a brand-new repo — nothing else exists to isolate from — costs nothing if wrong.
- Ruling: R1 CSP may add only what Tauri itself needs (ipc: and http://ipc.localhost in connect-src, font-src 'self' data:) — plan's literal CSP would break the app — if wrong, CSP is slightly wider than the plan text but still loads nothing remote.
- Ruling: R2 scaffold with create-tauri-app non-interactively in a temp dir and move files in, keeping existing docs/ and .gitignore — repo already has commits — none.
- Ruling: R3 if T3 finds paste-a-code unsupported, T8 omits sign_in_with_code and T9 offers paste-a-key only — plan step 4 of T3 says so — none.
- Ruling: R4 UiError.kind gains "keychain_unavailable" (retryable false, message "This computer has no keychain service.") returned by sign_in_with_key/sign_in_with_code; the key is held pending resolve_key_storage; T9 opens the dialog on it — spec requires the Linux choice on every sign-in path — if wrong, one extra error kind.
- Ruling: R5 send_message writes its model_id onto the chat row, so the chat remembers its model — spec/plan T11 say remembered per chat — none.
- Ruling: R6 StoredAttachment { name, mime, data_base64 } (all String). T8 persists attachments and sends text-only wire messages; T13 adds the parts conversion — smallest definition that satisfies both tasks — schema change if wrong.
- Ruling: R7 "pass on macOS, Windows and Linux in CI" cannot be met in-session: no remote, and pushing is the owner's call. Tests run locally on macOS; ci.yml is written but unexercised; Windows/Linux-only code paths (cfg) are compile-unverified — reported to owner at finish — cost: platform bugs found later on first CI run.
- Ruling: R8 interactive `pnpm tauri dev` walks and real OpenRouter sign-in are not done by subagents; implementers instead prove `cargo build` and `pnpm build` succeed. The manual walk is handed to the owner at finish — subagents have no GUI or account — cost: UI/integration bugs only a real run shows.
- Ruling: R9 package/crate name `beluga-desktop` (repo name) in package.json, Cargo.toml, main.rs stands — package identifiers need some name and renaming is a find-and-replace of one token — costs a three-file rename when the real name is chosen.
- Ruling: R10 inline pre-paint script replaced by public/appearance-init.js — CSP has no script-src so inline is blocked — none.
- Ruling: R11 a 401 while validating a newly pasted key (sign_in_with_key) does not emit auth://revoked or sign the user out — a rejected new key says nothing about the saved one — if wrong, one-line change.
- Ruling: R12 send_message returns Result<(), UiError> rather than () — a missing chat or DB error must reach the UI — ipc.ts consumers handle a rejected promise.
- Ruling: R13 sign-in gains command sign_in_code_url (auth URL without callback) for the paste-a-code path — plan had no way to obtain that URL — one extra command.
- Ruling: R14 Retry gets a core path: new command retry_turn(chat_id, model_id, on_event) that drops a trailing interrupted assistant message and re-runs the turn on stored history without storing the user message again; useChat.retry uses it (or resends the unsent pending text after a locally rejected send) — plan's "retry() resends the last user message" would persist duplicates the model then sees — costs one extra command.
- Ruling: R15 header shows "This chat: 1 request, $x" (singular) when n is 1 — the plan's "{n} requests" was a template, and "1 requests" is wrong English — if wrong, remove one conditional.
- Ruling: R16 free daily reset shown as 00:00 UTC in local time — docs say the counter covers "the current UTC day" but state no reset time — if wrong, the displayed reset time is off; the counter itself comes from the API.
- Ruling: R17 "Beluga contributors" in LICENSE and Cargo.toml authors stands — a copyright holder needs some name; changes with the product name — two-line edit.
- Ruling: R18 when storing the reply fails after a turn ran, the command returns Ok(status failed) rather than Err — Err would mark the stored user message "Not sent" and duplicate it on retry — a rare DB failure shows as a failed turn.

## Deferred minor findings

- Task 1: minor (deferred): vitest.config.ts not type-checked (tsconfig.node.json includes only vite.config.ts)
- Task 2: minor (deferred): no test for storage-throwing path or applyAppearance("light")
- Task 2: minor (deferred): JetBrains Mono only 400/500 weights imported
- Task 3: minor (deferred): "2xx but no key" fallback untested
- Task 3: minor (deferred): base_url with trailing slash yields //api
- Task 3: minor (deferred): raw response body in ApiError::Other.message — ensure never shown/logged verbatim (Task 7 maps to UiError "other" with {message}; trim there)
- Task 4: minor (deferred): response written without draining request; Windows may RST and drop the success page
- Task 4: minor (deferred): Url::join accepts absolute-form targets; stricter /callback prefix match possible
- Task 4: minor (deferred): cap path (>=8 stalled conns) untested; tests use http://localhost and rely on ::1 -> 127.0.0.1 fallback (unverified on Windows/Linux; same applies to real browsers)
- Task 5: minor (deferred): FileStore::set non-atomic, follows symlinks
- Task 5: minor (deferred): from_response keyword match runs on 200-char-truncated message
- Task 5: minor (deferred): ModelCache::save non-atomic
- Task 5: minor (deferred): Windows FileStore has no ACL restriction
- Task 5: minor (deferred): 429 Retry-After header not read (retry_after_secs always None)
- Task 6: minor (deferred): missing/non-numeric usage.cost becomes 0.0 (could under-report spend)
- Task 6: minor (deferred): string error code (e.g. "server_error") maps to status 500 -> Other; untested
- Task 6: minor (deferred): final [DONE] without trailing blank line ends as Network error
- Task 7: minor (deferred): clean-EOF-without-Done arm and cancel-while-connecting are untested
- Task 8: minor (deferred): concurrent sends in one chat interleave stored history (turn.rs)
- Task 8: minor (deferred): deleting a chat mid-turn makes send_message return a DB error
- Task 8: minor (deferred): no RAII cleanup guard on running-turn map
- Task 8: minor (deferred): stale loopback task completing can stop a newer pending sign-in (save_key should match id)
- Task 8: minor (deferred): paste-a-code pending sign-in never times out
- Task 8: minor (deferred): key_status with no key returns unauthorized without flag/event
- Task 8: minor (deferred): blocking keychain/SQLite/file calls on async runtime
- Task 8: minor (deferred): attachment-only first message leaves chat title empty
- Task 8: minor (deferred): commands/tests.rs is 634 lines
- Task 8: minor (deferred): deleting a chat lowers month_spend (plan-mandated; owner to decide)
- Task 9: minor (deferred): "keeps the brand panel navy" test asserts little in its theme loop
- Task 9: minor (deferred): dialog focus trap fails when both buttons are disabled (busy); background not inert
- Task 9: minor (deferred): auth finished {ok:false, reason:null} returns to idle with no message
- Task 9: minor (deferred): unsubmitted pasted key stays in state when entering waiting phase
- Task 9: minor (deferred): heading order h2 before h1; SignIn.test.tsx asserts literal "Beluga"
- Task 10: minor (deferred): Shell swallows rename/delete/listChats failures silently
- Task 10: minor (deferred): failed notice lost when switching chats away and back
- Task 10: minor (deferred): ConfirmDelete has no focus trap / focus return; Escape untested
- Task 10: minor (deferred): send_message can reject after the user message was stored (DB error) -> UI shows "Not sent." and retry duplicates
- Task 10: minor (deferred): TDD order not followed by implementer (tests written with code)
- Task 11: minor (deferred): reopening picker after a typed query loses the selected-row highlight (ModelPicker open/query effects)
- Task 11: minor (deferred): loadModels has no sequence guard; chat selected before list arrives does not get its own model_id
- Task 11: minor (deferred): .chip/.free are unscoped global class names in models.css
- Task 12: minor (deferred): footer briefly shows paid view while models load / no model chosen
- Task 12: minor (deferred): reset time text not recomputed after it passes; out-of-order keyStatus responses not discarded
- Task 12: minor (deferred): Shell.tsx JSX indentation/fragment nits; meaningless `description` option in a test query
- Task 13: minor (deferred): base64 payload fully decoded before the size check (no encoded-length pre-check)
- Task 13: minor (deferred): attachment name not sanitised in the "File:" line (newline injection)
- Task 13: minor (deferred): text-by-extension not UTF-8 checked client-side; chips lost when the core refuses after Send
- Task 13: minor (deferred): pending message shows no attachment chips / empty bubble; opening Settings drops an unsent draft
- Task 13: minor (deferred): image mime not allow-listed (image/svg+xml passes)
- Final: minor (deferred): Composer clears draft after awaiting onSend — text typed while createChat is pending is wiped
- Final: minor (deferred): lib.rs set_title(...)? aborts app setup if the cosmetic title call fails
- Final: minor (deferred): send_message returns Err if set_chat_model/get_messages fails after the user message is stored -> "Not sent." + duplicate on retry
- Final: minor (deferred): sign_in_start has no sign-in id on auth://finished; attachments base64 returned on every reload and all images resent every turn
