# Beluga: project work

Design, 2026-11-12. Builds on `2026-11-11-beluga-desktop-design.md` (the parent design) and `2026-11-11-beluga-commands-and-skills-design.md`, and on the shipped app (version 0.2.0). Where they disagree, this document wins for the features it covers.

Items marked **(owner)** were stated or approved by the product owner. Items marked **(my call)** were decided by the designer and can be overturned. The owner asked the designer to analyse the project, research the web and decide, so most details here are **(my call)**.

Research behind this design: `docs/superpowers/research/2026-11-12-project-work-research.md`. Several of its findings are marked unverified; the ones that matter are repeated under "Things nobody has verified".

## Intent

The owner installed 0.2.0, asked it to do a project, and got code suggestions in a chat bubble. They want what they get from Claude's command-line tool and desktop app: open a folder, say what to build, and the app itself creates and edits the files, runs the commands, reads the results and keeps going until the project works, "frontend, backend etc". **(owner)**

Version 0.2.0 was honest that it could not act. This step makes it act.

## Goals

1. In a chat with a folder open, the model works on that folder by itself: it reads and searches files, creates and edits files, runs commands, and loops until the task is done or it needs the user. **(owner)**
2. It asks before running a command or changing a file, the way Claude's command-line tool does, with a way to stop being asked. **(owner: "same way")**
3. The user sees what it is doing as it happens and can stop it at any moment. **(parent design, owner approved)**
4. It cannot run away with the user's free requests or money. **(parent design, owner approved)**
5. It works on macOS, Windows and Linux in the same release. **(owner, parent design)**
6. It works on the mix of free and paid OpenRouter models the owner uses, and is honest when a model is not up to it. **(owner: "mix")**

## Non-goals (this step)

- An operating-system sandbox. The approval prompt is the safety boundary, as the parent design decided. Research confirms there is no usable native sandbox on Windows, so a sandbox cannot be offered equally on all three systems yet. **(parent design)**
- Undo, checkpoints or rewind of file changes. The app tells the user that changes are real and suggests version control; it does not keep its own copies. **(my call)**
- The model choosing skills by itself, a skill catalog in the prompt, and helper agents working in parallel. Skills still run when the user picks them; they can now act. **(my call)**
- Tools that reach the web directly (fetch a page, search). The model can still reach the network through a command the user approves. **(my call)**
- MCP servers, git buttons, a file-tree or diff side panel, a terminal view, voice. **(my call)**
- A "Recommended" badge from an automated model test. The parent design wants one; it needs a paid test run against real models that no build session has had the means to do. This step ships the plain-language warnings and leaves the tested list for later. **(owner, 2026-11-12: "Later"; changes the parent design)**
- Working outside the opened folder with the file tools. They refuse; they do not prompt. **(my call, stricter than the parent design)**

## Anti-goals

- A command running, or a file changing, that the user did not allow.
- An approval card that shows something other than what will actually happen: a different path, a shortened command, a relative path that resolves somewhere else.
- A task that silently spends the day's free requests or more than the user's limit.
- A stopped or crashed task leaving the chat unusable.
- The model being told it can act when it cannot, or the reverse.
- Copying Anthropic's names, wording or look. Interaction patterns only.

## What exists (0.2.0), from a read of the code

- One request per turn. `Engine::run_turn` (`src-tauri/src/engine/mod.rs`) is the single boundary between the app and the model; `DirectEngine` sends one streamed request and returns one text. The parent design's plan to try the external "goose" engine first was never carried out and no trace of it exists. **This design builds the loop in-house behind the existing `Engine` trait and drops the goose spike. (my call)**
- The request sends only `model`, `messages`, `stream`, `usage`. The stream reader ignores tool calls and the finish reason. There is no tool role.
- A stored message is role, text, attachments, an interrupted flag and a kind (`message` or `note`). Nothing can hold a tool call or a result. Schema version 3.
- A chat can have a folder, chosen only through a dialog the core opens. `src-tauri/src/folder.rs` already refuses `..`, absolute paths, drive letters and links that leave the folder, and walks the folder respecting `.gitignore`. It is read-only and can only resolve paths that already exist.
- The web view has no file, shell or dialog permission. The only process the app starts is none at all.
- The model list knows which models support tools. The picker does not yet filter on it. The allowance gauge reads the key's free daily count. There is no request counter during a turn, no pause, no daily limit.
- With a skill active, the model is sent a note saying it cannot read files or run commands.

## How it behaves

### When the agent is on

A chat is a **project chat** when it has a folder open. In a project chat the model is given tools. In a chat without a folder nothing changes from 0.2.0. There is no separate mode switch: opening a folder is the switch. **(my call)**

The new-chat screen keeps its line saying which project is open and now says what the app may do there, exact copy: "{APP_NAME} can read files in this folder. It asks before changing a file or running a command." **(my call)**

With a folder open, the model picker lists only models that support tools, as the parent design says. If the chat's current model does not support tools when a folder is opened, the app says so and asks the user to pick one; it does not switch by itself. Requests in a project chat set OpenRouter's `provider.require_parameters` to true so a provider cannot silently drop the tools. **(parent design; the routing flag is my call)**

### The tools

Eight tools the model can call, plus three for long-running commands. Names are plain and snake_case; they are not Claude Code's names. **(my call, following the research's comparison of Claude Code, OpenCode, Goose, Cline and Aider)**

| Tool | What it does | Asks first? |
|---|---|---|
| `read_file(path, offset?, limit?)` | Returns numbered lines of a text file. At most 400 lines or 16,000 characters per call; says when more remain. | No |
| `list_dir(path?)` | Names, kinds and sizes in one directory. | No |
| `find_files(pattern, path?)` | Paths matching a glob, `.gitignore` respected, at most 200. | No |
| `search(pattern, path?, glob?)` | Regular-expression search of file contents, `.gitignore` respected; at most 100 matching lines, with path and line number. | No |
| `edit_file(path, old_text, new_text, replace_all?)` | Replaces exact text. Fails unless `old_text` occurs exactly once (or `replace_all` is true). The file must have been read in this chat since it last changed. | Yes |
| `write_file(path, content)` | Creates a file, or replaces a whole file. Replacing an existing file requires that it was read first. Parent folders are created. | Yes |
| `run_command(command, timeout_seconds?, background?)` | Runs one shell command in the project folder and returns its exit code and output. | Yes |
| `update_plan(items)` | Replaces the task's checklist (each item: text and one of pending, in progress, done). Shown to the user. | No |
| `command_output(id)` | New output and status of a background command. | No |
| `stop_command(id)` | Stops a background command. | No |

Edits are exact text replacement plus whole-file write. No diff or patch format: research found that weaker models miscount diff line numbers, and that exact-replace with a clear failure message is what every compared agent uses. When `old_text` is not found the result says so and shows the closest lines; when it is found more than once it says how many times. A match that fails only on line endings or trailing spaces is retried with those ignored, and applied only if that match is unique. **(my call)**

Paths given to file tools are relative to the project folder. A path that leaves it (by `..`, an absolute path, a drive or UNC path, a link, or a Windows stream name) is refused with a message the model can read; nothing is prompted. Checks are made on the real resolved path, by path component, and case-insensitively on macOS and Windows. A new file is checked through its nearest existing parent. Writing through a link is refused even when the link stays inside. **(my call)**

Files that usually hold secrets are not read without asking: `.env` and `.env.*` (except `.env.example`, `.env.sample`, `.env.template`), private keys (`*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`), and `.npmrc`, `.netrc`, `.pypirc`, `credentials*` files. Reading one shows a card, exact copy: "Let {APP_NAME} read {path}? Its contents will be sent to the model." with "Allow once" and "Don't allow". The search tools skip these files. **(my call)**

Binary files and files over 5 MB are refused by `read_file` with a plain message. Images are not read by tools in this step.

### Asking first

**Commands.** A command request shows as an amber card with the exact, complete command, the folder it runs in, and three choices, exact copy: "Run once", "Always allow `{rule}` in this project", "Don't run". **(parent design, owner approved the mockup)**

- "Don't run" opens a one-line field, "Tell it what to do instead (optional)", and the model receives that the user declined, with the note if given. **(owner: same as Claude's tool; wording my call)**
- The rule is the program name plus its first sub-command when the first argument is a plain word (`npm install`, `git status`, `cargo test`); otherwise the program name alone. Rules belong to one project folder and are listed in Settings under the project, where each can be removed. **(parent design)**
- The "Always allow" choice is not offered, and saved rules never match, when the command contains a pipe, `&&`, `||`, `;`, `&`, a redirect, a sub-shell or substitution, a newline, or `sudo`; when the command cannot be parsed; or when the program is an interpreter or launcher that can run anything (`sh`, `bash`, `zsh`, `fish`, `cmd`, `powershell`, `pwsh`, `python`, `python3`, `node`, `deno`, `bun`, `ruby`, `perl`, `php`, `env`, `xargs`, `npx`, `pnpx`, `bunx`, `uvx`, `eval`, `exec`) or a remover or mover of files (`rm`, `rmdir`, `del`, `rd`, `mv`, `move`, `dd`, `format`, `mkfs`, `chmod`, `chown`, `git push`, `git reset`, `git clean`, `git checkout`, `git restore`). Those always prompt. The builder may extend this list and must not shorten it. **(parent design for the first half; the lists are my call)**
- When the command text mentions a path outside the project (an absolute path, `~`, or `..` that climbs out), the card adds a line, exact copy: "This command refers to a place outside the project folder." This is a hint from reading the text, not a guarantee; the card says nothing that implies commands are confined. **(parent design)**
- The first command card in each project carries one extra line, exact copy: "Commands run on your computer with your permissions. {APP_NAME} cannot undo them." **(my call)**

**File changes.** The first time the model wants to change a file in a chat, a card shows the path and the change (lines removed and added; for a new file, its first 40 lines and its length) with, exact copy: "Allow once", "Allow all edits in this chat", "Don't allow". After "Allow all edits in this chat", later edits and new files in that chat happen without a card and show in the trail. The choice is stored with the chat and shown as a small chip "Edits allowed" near the message box, which the user can click to switch off. **(my call. The parent design's default was to allow edits without asking; the owner then said to follow Claude's command-line tool, which asks. Owner said "ok" when asked which; this is the designer's reading.)**

"Don't allow" offers the same optional note as "Don't run".

**Answering.** A card stays until answered; the task waits and spends nothing while it waits. Only one card is open at a time. Stop, or Esc, cancels the task and counts as declining. If the app is closed while a card is open, the task ends as interrupted and the action is recorded as not run.

**Sending a message while a card is open.** The message answers the card; it is never queued behind it and never cancels the task. On a command, edit or secret-file card it means "Don't run" / "Don't allow" with the message as the note, and the task carries on with that. On the "Keep going?" card it means "Keep going", and the message is given to the model as the user's next instruction. The message is shown in the conversation as the user's. A message sent while a task is running with no card open is queued as in 0.2.0. **(my call, following Claude's command-line tool, which the owner asked to match)**

**Who can answer (a known weakness, my call).** The card is drawn in the web view and the answer travels to the core as a command, so a web view that had been taken over could answer for the user. Version 0.2.0 kept folder and file choices in dialogs opened by the core for exactly this reason. Doing the same for every command approval would mean a system dialog per command, which is not the approved design and would be unusable. This design accepts the in-app card and reduces the exposure: the web view keeps its strict content policy, model and file text is only ever rendered as text, and the core, not the interface, decides what needs approval, builds the card's contents, and only accepts an answer for the approval it is currently waiting on. An optional setting "Confirm commands in a system dialog" is left as later work.

### The trail

While a task runs, the conversation shows, in order **(parent design for the idea; details my call)**:

- The model's text, as now.
- Each tool call as one quiet indented line: "Read src/app.ts", "Searched for "useChat"", "Edited src/app.ts (+12 −3)", "Created server/index.js", "Ran `npm install` (exit 0, 14 s)". Failed and declined actions are marked. A line opens to show the detail (the change, or the command's output).
- The checklist from `update_plan`, pinned above the message box while the task runs, each item with its state.
- A running command shows its last lines as they arrive.

The header's "Chat is {n}% full" meter and the per-chat request and cost line keep working and update during a task, not only at its end.

### Commands

- A command runs in the project folder through one shell: on macOS and Linux `/bin/bash` if present, else `/bin/sh`; on Windows `pwsh` if present, else `powershell.exe`. The model is told which system and shell it is on. **(parent design asked for a documented Windows choice; this is it, my call)**
- It inherits the app's environment, with the variables the app itself uses for its key or proxy removed. It is given no terminal: a command that waits for typed input sees end of input and usually fails; the result tells the model to use non-interactive flags. **(my call)**
- Default time limit 120 seconds, at most 600; the model may ask for longer within that. At the limit the command and everything it started is stopped and the result says so. **(my call, Claude Code's defaults)**
- Output returned to the model is at most 12,000 characters: the first 2,000 and the last 10,000, with a line saying how much was left out. The user can see up to 200,000 characters in the trail; beyond that it is cut. **(my call)**
- Stop cancels the running command and everything it started, on all three systems (process group on macOS and Linux, a job object on Windows), within two seconds.
- **Background commands.** Building a frontend and a backend means starting servers that never exit. `run_command` with `background: true` returns at once with an id; `command_output` reads what it has printed; `stop_command` stops it. Running background commands are listed in a strip above the message box ("Running: `npm run dev`", with Stop and a way to see output). They are approved like any command. They stop when the user presses Stop on them, when the chat is deleted, when the folder is closed or changed, and when the app quits; they keep running between tasks in the same chat. At most 4 per chat. **(my call)**

### Guards

- **Keep going?** Every 25 requests in one task the task pauses with a card, exact copy: "{APP_NAME} has made {n} requests on this task. {status}" with "Keep going" and "Stop here", where status is "{m} free requests left today." or "Spent ${x} today." **(parent design)**
- **Free requests.** At 0 free requests left on a free model the task pauses with its work saved, and the existing low-allowance notice offers paid tool-capable models with prices. It never switches model by itself. **(parent design)**
- **Daily limit for paid models.** Default $1.00, changeable in Settings (minimum $0.10; "No limit" is allowed and says what it means). Reaching it pauses the task the same way. The day is the UTC day, matching OpenRouter's free counter. Spend is the app's own record of what requests cost. The limit covers every paid request the app makes, not only project tasks: spend from ordinary chats and `/compact` counts towards it, and once it is reached a send on a paid model in any chat is refused before any request is made, exact copy: "You have reached today's spending limit of ${x}. You can change it in Settings." Free models are never blocked by it. **(parent design; details my call)**
- **Going in circles.** The task stops and tells the user when the model makes the same failing call three times in a row, or five tool calls in a row that cannot be understood. **(my call)**
- **Rate limits.** A "too many requests" answer is retried after the wait the server names (up to 60 seconds, at most 3 times per task), with the trail saying it is waiting. **(my call)**
- **Full chat.** Before each request the app estimates the size of what it will send. Over 70% of the model's context it replaces the contents of the oldest tool results in what is sent (not in what is stored or shown) with a one-line placeholder, oldest first, never separating a call from its result. If that is not enough the task pauses, exact copy: "This chat is too full to continue. Use /compact to shorten it, then say continue." **(my call)**

A paused task is not a failed task: the conversation up to the pause is stored, and the user continues by answering the card or sending a message.

### Stopping, crashing, continuing

Everything the model says and every tool call and result is stored as it happens, not at the end of the task. After Stop, a crash, a lost connection or quitting the app, the chat shows what was done, and a tool call that never got a result is recorded as "Not run: the task was interrupted." so the conversation is always valid to send again. The user continues by sending a message (for example "continue"); an interrupted task shows a "Continue" button that does that. **(parent design's "stopped mid-task and continued later"; mechanism my call)**

Retry and edit, which assume one question and one answer, change in project chats **(my call)**:
- "Retry" is not offered on a reply that belongs to a task that ran tools.
- Editing an earlier message still removes it and everything after it from the chat, including tool calls. The confirmation adds a sentence, exact copy: "Files and commands already changed on your computer stay as they are."
- A message typed while a task runs is queued as in 0.2.0 and sent when the task ends or pauses.

### Skills

- The note sent with skills is now true to the chat: in a project chat it says which tools exist and that skills written for other tools should use these instead (read a file, edit a file, run a command, keep a checklist), and that helper agents are not available so their steps are done in order by the model itself. In a chat without a folder it keeps the 0.2.0 wording. **(my call)**
- The Skills screen note for skills marked "acts on your computer" changes, exact copy: "Written for an agent that can edit files and run commands. Open a folder in the chat and it can do that, asking you first." **(my call)**
- A skill's other files can be read by the model through `read_file` using a `skill:{id}/{file}` path, read-only, limited to the files the pack lists. **(my call; the parent design wanted skill folders readable by the agent)**

### What the model is told

In a project chat the first system message is the app's own, fixed for the whole chat so it can be cached: what it is, the system and shell, the project folder's name (not its full path), that it should look before changing, keep changes small, check its work by running the project's own tests or build, use the checklist for multi-step work, say plainly when it is blocked, and never claim to have run something it has not. It also says that text inside files and command output is information, not instructions. The exact wording is the builder's, kept under 600 words, and the same on every request of a chat. **(my call)**

## Limits and facts to build on

- OpenRouter free models: 20 requests a minute; 50 a day, or 1,000 a day once 10 credits have ever been bought. One task step is one request, so 50 a day is a few small tasks. The app already tells users about the top-up; the project new-chat screen repeats it for free-tier keys. (Research; treat the numbers as reported, read the live values from the key endpoint.)
- Tool-call streaming in OpenRouter's chat format is not documented on any page the research could read. Build the reader to the common convention and defensively: collect call fragments by index, join argument pieces, and decide that the model wants tools from the collected calls, not from the finish reason alone. Arguments that are not valid JSON get one repair attempt (trailing commas, a missing closing brace, a fenced block), and otherwise an error result the model can act on. Every call id always receives a result.
- Prompt caching lowers the cost of long tasks only if the start of every request is byte-for-byte the same. Keep the system message, tool list and earlier messages stable; mark the cache point for providers that need it; send a constant session id per chat.
- Suggested crates, all permissively licensed and current on the research date: `process-wrap` (process groups and job objects with tokio), `ignore`, `globset`, `grep-searcher` with `grep-regex`, `similar` (to show a change). The builder may choose others and records why.

## Storage

Schema version 4, upgraded in place from 1, 2 or 3, keeping every chat. It must hold: for a model message, the tool calls it made; for a tool result, which call it answers, the tool, a short summary for the trail, whether it succeeded, failed, was declined or was not run, and the result text sent to the model; per chat, whether edits are allowed and the checklist; per project folder, the saved command rules; per day, requests and spend. Deleting a chat must not lower the day's or month's recorded spend (a gap carried from 0.1). How this is laid out is the builder's choice.

## Safety summary

What the app guarantees: no command runs and no file changes without an approval the user gave (once, by rule, or for the chat); file tools never touch anything outside the opened folder; the card shows the real path and the whole command; the key never reaches the model, a command's environment, or the web view.

What it does not guarantee, and says so: an approved command can do anything the user can do, anywhere on the computer, including using the network. File contents and command output are sent to the model provider. A model can be misled by text inside a project's files; the approvals are the defence.

## Testing the builder must do

- Path safety, on all three systems in CI: every escape named above, for read, write and edit, including a link created after the check, case tricks on macOS and Windows, and Windows drive, UNC and stream paths.
- The rule matcher: a table of commands and whether a saved rule may match, covering every separator and every program on the never-list, for both shell families.
- The loop against a scripted fake model: several steps, two calls in one reply, a call with broken arguments, an unknown tool, an approval declined with a note, Stop during a command, Stop during an approval, the 25-request pause, the free limit reaching 0 mid-task, the daily limit, a rate limit, a full chat, and the three-identical-failures stop.
- Commands for real in CI on all three systems: output capture, exit codes, the time limit, stopping a command that started children, a background command read and stopped, end of input for an interactive command.
- An upgrade from each earlier schema version with chats present.
- A stored conversation from an interrupted task can always be sent again.
- No approval answer is accepted for an approval that is not the one waiting.

## Things nobody has verified

1. **No part of this app has ever been run against a real model with tools.** Build sessions have had no OpenRouter key. The stream format, which free models actually manage a multi-step task, and the effect of the routing flag are all unknown until someone runs it. The plan must include a small live check that reads a key from an environment variable and runs a fixed task (read a file, edit it, run a command, report) against at least two free and two cheap paid tool-capable models, skipped when no key is set. **The owner will supply a key for that run when the build reaches it (owner, 2026-11-12).** The live check is a release gate: 0.3.0 is not tagged until it has run and what it found is fixed or written down. The plan keeps room after it for those fixes. The key is read from the environment only, is never written to a file, a log, a test fixture or a commit, and the streams recorded from the run are saved as test fixtures with the key and any account details removed.
2. Whether any free model is good enough. If none is, project work still ships and the app says so plainly when a task keeps failing, exact copy: "This model is struggling with this task. A stronger model usually does better." with the picker's cheapest paid tool-capable models.
3. Windows behaviour of everything involving a shell is tested only by CI, not by a person at a Windows machine.

## Left to the builder

The layout of storage and events; the exact system message; crate choices; how the trail's detail view looks; the order of work. The exact copy quoted here, the lists marked "must not shorten", the limits with numbers, and the safety guarantees are fixed.

## Version

0.3.0. The README's roadmap marks stage 3 released and states the safety summary in plain words.
