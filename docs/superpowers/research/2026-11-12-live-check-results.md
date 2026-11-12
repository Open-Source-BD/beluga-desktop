# Live check results: project work against real models

Date: 2026-11-12. Machine: macOS (Darwin 25.6), bash. Test: `src-tauri/tests/live.rs` (commit 32c9f70), run as

```
OPENROUTER_API_KEY_FILE=... cargo test --manifest-path src-tauri/Cargo.toml --test live -- --ignored --nocapture --test-threads=1
```

The test uses the app's real `DirectEngine`, the real task loop (`turn::send_message` → `run_task`), the real dispatcher and tools, an in-memory store and a temp project folder. A local relay passes every request on to OpenRouter unchanged and enforces the budget. In the recorded runs every approval card was answered `Once` automatically. The approval policy was tightened after the run (commit e96f338). The test now approves only the task's own edit of `notes.txt` and its own script or a plain read of a project file, inside the project and not in the background. It declines every other command and every secret file. The new policy is checked offline only (`the_approval_policy_allows_only_the_task_itself`); it has not been through a live run. The task: *"Read notes.txt, change alpha to beta, run the count script, and tell me what it printed."* The project held `notes.txt` (`alpha`), `count.sh` and `count.ps1`, which print `lines: 3`.

A run passes when `notes.txt` contains `beta`, the final reply contains `lines: 3`, the task used at most 12 requests, every call has exactly one result, and the stored conversation is well formed.

## How the models were picked

From the live `GET /api/v1/models` list, only models whose `supported_parameters` include `tools`. Free models are taken by largest context and paid models by lowest prompt price, with ties broken by id. A model this account could not use at all was skipped and the next one taken. "Could not use" means its first request was refused before any reply: 404 or "no endpoints found", 403 (the endpoint is closed to this app), or 402 (needs more credit than the account has).

There were two runs. The first counted the 402 and 403 refusals as failed models. After I widened the skip rule, the second run tried only the replacement models; the two models that had already run were passed in through `LIVE_ALREADY_RUN` rather than run again.

## Results

| Group | Model (provider) | Result | Requests | Reported cost | Wall time |
|---|---|---|---|---|---|
| free | `baidu/ernie-5.5-flash-preview:free` (Baidu) | **pass** | 5 | $0 | 21.0 s |
| free | `nvidia/nemotron-3-ultra-550b-a55b:free` (Nvidia) | **pass** | 5 | $0 | 35.8 s |
| paid | `inclusionai/ling-3.0-tiny` (Ant Digital) | **pass** | 11 | $0.000320 | 47.3 s |
| paid | `meta-llama/llama-3.1-8b-instruct` (Groq) | **fail** (the model) | 3 | $0.000286 | 12.4 s |
| free | `thinkingmachines/inkling-2-small:free` | skipped: 403 | 1 per run | $0 | |
| paid | `z-ai/glm-5.2` | skipped: 402 | 1 per run | $0 | |
| paid | `deepseek/deepseek-v4.1-flash-1017:batch` | skipped: 404 | 1 per run | $0 | |

Four models ran and three passed. In all four, every call got exactly one result and the stored conversation was well formed.

**Tools called, in order:**

- **ernie:** read_file, find_files, read_file, read_file, edit_file, run_command. Cards: edit, command. Every call succeeded.
- **nemotron:** list_dir, read_file, edit_file, run_command. Cards: edit, command. Every call succeeded.
- **ling:** read_file, find_files, edit_file, then three rounds of read_file on `.count.ps1` and `.count.sh`. All six of those reads failed with "There is no file or folder at .count.ps1." The model wrote those paths itself: the arguments were `{"path": ".count.ps1"}`. In between it called find_files and list_dir. Then it ran `cd /tmp && echo 'alpha' > notes.txt && echo 'beta' > notes.txt && cat notes.txt`, read `count.ps1` and `count.sh` correctly, and ran `cd . && bash count.sh`. Cards: edit, command, command. It passed, but it was close to the limit: 11 of 12 requests. The `/tmp` command wrote `/tmp/notes.txt` outside the project. The automatic `Once` of the recorded run approved it; a user would have seen the command card first. I deleted that file after the run. The tightened policy would decline this command, as well as `cd . && bash count.sh`, which has shell syntax the policy does not allow. So under that policy this run would have gone differently.
- **llama:** a single reply held five calls: read_file twice (the second with offset 401, which failed as past the end), edit_file (which succeeded), read_file, and run_command `bash scripts/count.sh`. The run failed with exit code 127 because that path was the model's guess; it never listed the folder. Its next reply called stop_command on a `count_server` that never existed (it failed). Its final reply said the script does not exist. This is a fault of the model, so I recorded it and changed nothing.

**Totals:** 30 chat requests across both runs (budget 60) and $0.000606 total reported cost (budget $0.50). The key endpoint's usage rose by $0.000300 between the start of the first run and the end of the second. It lags behind the per-request costs and had not caught up when the run ended.

## What was wrong in the app

The request and stream code handled every real stream correctly, so nothing needed fixing there. Two small fixes elsewhere came out of the run:

- **A 402 the balance cannot cover was shown as an empty balance** (commit f7434b2). The app sends no `max_tokens`, so OpenRouter reserves a model's full output. For `z-ai/glm-5.2` it refused with 402: "You requested up to 131072 tokens, but can only afford 8450", even though the balance was not empty. A 402 whose message contains "can only afford" now becomes the `low_credit` error: "Your balance is too low for this model's full reply. Add credit or pick a cheaper model." Any other 402 keeps the empty-balance message. The app still sends no `max_tokens`; that remains the owner's decision.
- **Commands inherited `OPENROUTER_API_KEY_FILE`** (commit 9d328ee). The app already removed `OPENROUTER_API_KEY` from every command's environment, but not this variable, which names a file holding a key. With automatic approval, a model's command could have read the key through it. It is now removed too. The live test also clears both variables from its own process as soon as it has read the key (commit e96f338).

### What the providers did (observed on four providers on one day; not settled)

- **Assistant content next to `tool_calls`.** The app sends an array of text parts when the reply had text, and `null` when it had none. Every one of these requests was answered with 200:
  - **Baidu (ernie):** sent both an array and `null`.
  - **Ant Digital (ling):** sent both an array and `null`.
  - **Groq (llama):** sent both an array and `null`.
  - **Nvidia (nemotron):** sent `null` only. Its replies with calls had no text, so the array form was never sent to Nvidia.
- **`provider.require_parameters: true` and `session_id`.** These were sent on every request. No provider that ran refused a request because of them.
- **What the committed fixtures show** (`src-tauri/tests/fixtures/live/`, checked by the `live_*` tests in `openrouter/stream.rs`):
  - several calls in one reply: two from Baidu and from Ant Digital, five from Groq
  - text followed by calls in the same reply: Baidu
  - `reasoning` / `reasoning_details` chunks: Nvidia and Ant Digital (none in the Baidu or Groq fixtures). The app ignores them.
  - `: OPENROUTER PROCESSING` comment lines: Nvidia
  - every fixture ends with two `finish_reason: "tool_calls"` chunks, the second carrying the usage

  The ids in the fixtures are placeholders (`gen-live`, `call_live_N`, `fp-live`), so the fixtures do not show the providers' own id formats.
- **Observed during the run but not in the committed fixtures** (seen only in recordings that were not saved):
  - Groq's call ids were not of the form `call_…` (short random strings); the app took them as they came.
  - Groq's final text reply ended with two `finish_reason: "stop"` chunks, the second carrying the usage.
  - A later Baidu reply with calls (not the one saved as its fixture) carried `reasoning` chunks.
- **Finish reasons.** Only `tool_calls` and `stop` were seen. `finish_reason: "length"` never occurred, so what the app does with truncated tool arguments is still untested against a real provider.

## What remains

- **No `max_tokens`.** Because the app sends none, a low balance still blocks models with a large output and a high completion price, even though the balance could pay for a short reply. The message is now accurate (see above). Sending a `max_tokens` would avoid the refusal, but it is a design choice (truncated replies, `length` finishes) left to the owner.
- **Free endpoints gated to known apps.** `thinkingmachines/inkling-2-small:free` answered 403 "only available on agentic harnesses… listed on https://openrouter.ai/apps". OpenRouter recognises apps by their attribution headers. The app sends `X-Title` and, by design, no `HTTP-Referer`. As things stand, some free models cannot be used from the app at all.
- **The cheapest-paid rule picks unusual models.** The rule as written picked a batch-only model (404 on chat completions) and a model with near-zero prompt price but a high completion price. Useful paid models were reached only after skipping those.
- **Not covered:** Windows (PowerShell, `count.ps1`), Linux, long tasks, the 25-request pause, rate-limit waits, and `finish_reason: "length"`.

## What this does and does not show

It shows that on one day, on macOS, the app's real request and stream code ran multi-step project tasks against four real models from four providers. Three of them read, edited, ran a command and reported the result within the limits, and every stored conversation stayed valid. It does not show behaviour on Windows or Linux, on long or complex tasks, on other models or providers, or over time as providers change.
