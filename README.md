<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="Beluga icon">

# Beluga

**A desktop AI chat and coding assistant that runs on your own OpenRouter account.**
No subscription. Start on free models, pay only for what you use.

[![CI](https://github.com/shamirul-007/beluga-desktop/actions/workflows/ci.yml/badge.svg)](https://github.com/shamirul-007/beluga-desktop/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/shamirul-007/beluga-desktop?include_prereleases&label=release)](https://github.com/shamirul-007/beluga-desktop/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-informational)

[Download](#download) · [First run](#first-run) · [Features](#features) · [Roadmap](#roadmap) · [Build from source](#build-from-source)

</div>

---

> **Early preview.** This is version 0.3: the chat foundation, commands and skills, and now **project work**, where the app reads and edits files and runs commands in a folder you open. Project work is new in 0.3. It is tested by an automated suite on macOS, Windows and Linux against a simulated model, and a [live check](docs/superpowers/research/2026-11-12-live-check-results.md) ran a small task on four real models on macOS, three of which completed it; it has not been tried with every model, or with real models on Windows and Linux. It is young software: expect rough edges and please [report what you find](https://github.com/shamirul-007/beluga-desktop/issues). Before you use project work, read [What it can and cannot protect you from](#what-it-can-and-cannot-protect-you-from). "Beluga" is a working name and may change.

## Why Beluga

Good AI coding tools sit behind monthly subscriptions that many developers cannot afford. Beluga takes a different route:

- **Bring your own account.** Beluga talks to [OpenRouter](https://openrouter.ai) with a key that belongs to you. There is no Beluga account and no Beluga server.
- **Free to start.** OpenRouter offers free models. Beluga puts them first and always shows how many free requests you have left today.
- **No surprises.** Every chat shows its request count and cost. Beluga never switches you to a paid model by itself.

The goal for later versions is to give a beginner on a free model the working habits of a senior developer: plan before coding, test what is written, review the result. See the [roadmap](#roadmap).

## Features

What works in this version:

| | |
|---|---|
| **Sign in with OpenRouter** | One button opens OpenRouter in your browser and returns a key to the app. You can also paste a key or a one-time code. |
| **Secure key storage** | Your key is kept in the operating system keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service). It never reaches the interface layer and is never logged. |
| **Streaming chat** | Replies appear as they are written. Stop a reply, retry an interrupted one, rename and delete chats. History is saved on your computer. |
| **Live model picker** | The model list is fetched from OpenRouter, grouped into Free and Paid, with prices per million tokens. No model names are hard-coded. |
| **Free-request gauge** | Fifty marks, one spent per request, with the reset time and your spend this month. It warns at ten left and explains your options at zero. |
| **Attachments** | Attach text files and, for models that can read them, images. Paste an image, drop files on the conversation, or paste a long text (4,000 characters or more) as a file chip. |
| **`/` menu** | Type `/` in the message box for commands (`/new`, `/model`, `/skills`, `/usage`, `/compact`, `/export`, `/settings`, `/help`) and skills. Picking a skill switches it on for that chat. |
| **`@` files** | Choose a project folder for a chat, then type `@` to attach a file from it. Files are read by the app and never from outside the chosen folder. |
| **Project work** | Open a folder in a chat and the model works on it by itself: it reads, searches, creates and edits files, runs commands, keeps a checklist, and carries on until the task is done or it needs you. It asks first; see [Project work](#project-work). |
| **Skills screen** | Turn skills and whole packs on or off, read what a skill does, add your own from a folder, a zip or a GitHub address, and install optional extra packs. |
| **Message actions** | Copy any message, copy a code block, retry a reply (optionally with another model), and edit and resend a message. Up arrow in an empty box edits your last message; a message sent while a reply is running waits in line; Esc stops a reply. |
| **Bundled skill packs** | Fifteen skill packs for planning, test-first work, debugging, review and design are included; 21 skills are on by default. See [Bundled skill packs](#bundled-skill-packs). |
| **Light, dark and system modes** | Follows your operating system by default. |

## Project work

A chat is a **project chat** when it has a folder open. In a project chat the model is given ten tools. A chat without a folder works as it did in 0.2, except that the daily spending limit for paid models now applies there too. Only models that support tools can be used in a project chat; the model picker lists those.

| Tool | What it does | Asks first? |
|---|---|---|
| `read_file` | Reads numbered lines of a text file: at most 400 lines or 16,000 characters per call. Files over 5 MB and binary files are refused. | No (yes for secret files) |
| `list_dir` | Lists names, kinds and sizes in one folder. | No |
| `find_files` | Finds paths by pattern, at most 200. | No |
| `search` | Searches file contents with a regular expression, at most 100 matching lines. | No |
| `edit_file` | Replaces exact text in a file. The file must have been read first. | Yes |
| `write_file` | Creates a file, or replaces a whole file that was read first. | Yes |
| `run_command` | Runs one shell command in the folder and returns its exit code and output. | Yes |
| `update_plan` | Keeps a checklist, shown above the message box while the task runs. | No |
| `command_output`, `stop_command` | Read the output of, or stop, a background command. | No |

**Approvals.** A command shows a card with the whole command, its size and the folder it runs in, and three choices: **Run once**, **Always allow** a simple form of it in this project, or **Don't run** (with an optional note telling the model what to do instead). A file change shows the path and the change, with **Allow once**, **Allow all edits in this chat** or **Don't allow**. Once all edits are allowed, an "Edits allowed" chip near the message box shows it and switches it off; it is also switched off when the chat's folder is closed or changed. A card waits until you answer; the task spends nothing while it waits. A new card ignores clicks for half a second, and its buttons stay disabled until a command too long for its box has been scrolled to the end. A command longer than 4,000 characters or 50 lines, or containing hidden or control characters, is refused outright, without a card. Saved rules are listed per project in Settings, where each can be removed.

**The trail.** Each action appears in the conversation as one line ("Read src/app.ts", "Edited src/app.ts (+12 -3)", "Ran `npm install` (exit 0, 14 s)") that opens to show the change or the output. Stop ends the task at any moment, and Continue picks it up again after a stop, a crash or a closed app. Everything is stored as it happens.

**Commands.** On macOS and Linux a command runs in `/bin/bash` (or `/bin/sh` if there is no bash). On Windows the shell is PowerShell (`pwsh` if installed, otherwise `powershell.exe`), so commands are PowerShell commands. A command gets no keyboard input, so one that waits for typing fails. The default time limit is 120 seconds and the most the model can ask for is 600. The model sees at most 12,000 characters of output (the first 2,000 and the last 10,000); you can see up to 200,000. A command can run in the background (for a server that never exits); up to 4 per chat run at once, they are listed above the message box, and each has its own Stop.

**Guards.**

- Every 25 requests in one task it pauses and asks "Keep going?" with **Keep going** or **Stop here**.
- At zero free requests on a free model it pauses with its work saved. It never switches models by itself.
- A daily spending limit for paid models (default $1.00, minimum $0.10, or no limit) is set in Settings. It counts every paid request the app makes, not only project tasks.
- It stops when the model makes the same failing call three times in a row, or five tool calls in a row that cannot be understood, and retries a "too many requests" answer after the wait the server names (up to 60 seconds, 3 times per task).
- When a chat grows past 70% of what the model can hold in one chat it shortens old tool results in what it sends, and if that is not enough it asks you to use `/compact`.

If a model struggles, a stronger one usually does better. Which free models can manage a multi-step task is not yet known; this is why a tested "Recommended" list is still on the roadmap.

## What it can and cannot protect you from

Read this before you point project work at a folder you care about. Beluga asks before it acts, but **asking is the only safety boundary: there is no sandbox**.

**What Beluga guarantees**

- No command runs and no file changes without an approval you gave: once, by a rule you saved, or "all edits in this chat".
- The file tools never read or write outside the folder you opened (apart from reading the files of a skill you have installed). They refuse; they do not ask. The check uses the real resolved path, so `..`, absolute paths, drive and network paths and links that leave the folder do not work. They also never read, list, search or write Beluga's own data folder (its database, key file and installed skills), even when the folder you opened contains it.
- A card shows the whole command and the real path, not a shortened or relative version.
- Your OpenRouter key is never given to the model or to a command's environment.
- Files that usually hold secrets (`.env`, private keys, `.npmrc`, credential files, and anything inside `.git`) ask before they are read, and the file tools cannot change them at all.

**What Beluga does not protect you from**

- **An approved command can do anything you can do on your computer.** It can read and delete files outside the folder, install software and use the network. The card's note "This command refers to a place outside the project folder" comes from reading the command's text; it is a hint, not a guarantee. Read each command before you approve it.
- **Secret files are not safe from commands you approve.** The file tools ask before reading `.env` or `.git`, but a command you approve can read them.
- **File contents, file names and command output are sent to the model provider you chose**, as part of the conversation. Do not open a folder whose files must not leave your computer.
- **A model can be misled by text inside a project's files**, such as a downloaded README or a comment that tells it to run something. Beluga tells the model that such text is information, not instructions, but that is not a guarantee. Your approvals are the defence, so read the cards.
- **"Always allow" covers only simple forms of a command**, and the rule still runs whatever the project's own configuration makes that command do. `npm test` runs the `test` script in `package.json`; a build runs the project's build scripts; git commands use the repository's git configuration. Anything with a pipe, `&&`, `;`, a redirect, quotes or variables, and interpreters such as `bash`, `python` or `node`, always ask.
- **"All edits" plus a saved rule can run unreviewed code.** If you allow all edits in a chat and have saved a rule such as `npm test`, the model can change what that command does (for example the test script in `package.json`) and then run it without a card. If you saved a rule, turn off "Edits allowed" or review the changes.
- **Stop ends a command and everything it started**, except a process that deliberately runs itself separately from the app (a daemon) on macOS and Linux. Background commands end when the app quits normally; if the app is killed outright they keep running on macOS and Linux (they are ended on Windows). A server must be started directly, not with `&`.
- **There is no undo.** Changes are real and Beluga keeps no backup copies you can restore from. Use version control, and commit before you start a task.
- **The daily spending limit is a guard, not a hard cap.** Several chats working at once can each go one request past it, and when a provider reports no cost for a request Beluga records an estimate from token counts and the model's listed prices.

## Download

Get the installer for your system from the **[latest release](https://github.com/shamirul-007/beluga-desktop/releases)**.

| System | File to download | Notes |
|---|---|---|
| **macOS**, Apple silicon (M1 and later) | `Beluga_<version>_aarch64.dmg` | macOS 11 or later |
| **macOS**, Intel | `Beluga_<version>_x64.dmg` | macOS 11 or later |
| **Windows** 10 and 11, 64-bit | `Beluga_<version>_x64-setup.exe` | An `.msi` is also provided |
| **Linux**, Debian and Ubuntu | `Beluga_<version>_amd64.deb` | Needs WebKitGTK 4.1 |
| **Linux**, Fedora and openSUSE | `Beluga-<version>-1.x86_64.rpm` | Needs WebKitGTK 4.1 |
| **Linux**, any distribution | `Beluga_<version>_amd64.AppImage` | Portable, no install |

## First run

The installers are **not code-signed yet**, so macOS and Windows show a warning the first time. This is expected for an early open-source build. If you would rather not trust a download, you can [build from source](#build-from-source).

<details>
<summary><b>macOS</b></summary>

1. Open the `.dmg` and drag Beluga into Applications.
2. Open Beluga. macOS will say it cannot verify the developer.
3. Go to **System Settings → Privacy & Security**, scroll down, and choose **Open Anyway**.

If macOS says the app "is damaged and can't be opened", remove the download quarantine flag and open it again:

```bash
xattr -cr /Applications/Beluga.app
```

</details>

<details>
<summary><b>Windows</b></summary>

1. Run the `-setup.exe` installer.
2. If Microsoft Defender SmartScreen appears, choose **More info**, then **Run anyway**.

Beluga uses the WebView2 runtime, which is already part of Windows 11 and current Windows 10. The installer fetches it if it is missing.

</details>

<details>
<summary><b>Linux</b></summary>

```bash
# Debian, Ubuntu
sudo apt install ./Beluga_<version>_amd64.deb

# Fedora
sudo dnf install ./Beluga-<version>-1.x86_64.rpm

# AppImage
chmod +x Beluga_<version>_amd64.AppImage
./Beluga_<version>_amd64.AppImage
```

Beluga stores your key through the Secret Service API (GNOME Keyring, KWallet). On a desktop without one, Beluga tells you and lets you choose between keeping the key until you quit or saving it in a file only your account can read.

If the window is blank on an NVIDIA card, start the app with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.

</details>

## Getting started

1. **Create a free OpenRouter account** at [openrouter.ai](https://openrouter.ai) if you do not have one.
2. **Open Beluga** and choose **Sign in with OpenRouter**. Approve the request in your browser and you are returned to the app.
3. **Pick a model.** Beluga selects a free model for you; change it from the chip in the message box.
4. **Start a chat.**

### About the free allowance

OpenRouter sets the limits on free models, not Beluga:

| Your OpenRouter account | Free requests per day |
|---|---|
| Has bought fewer than 10 credits in total | 50 |
| Has bought 10 credits or more, once | 1,000 |

A one-time top-up of $10 therefore raises the daily free limit twentyfold, and the credit itself stays available for paid models. The gauge in the sidebar shows what you have left and when it resets.

## Privacy and security

- **No telemetry, no analytics, no Beluga server.** The app contacts `openrouter.ai`. It contacts GitHub (`github.com`, `codeload.github.com`, `api.github.com`) only when you install an extra skill pack or add skills from a GitHub address, and then only to download what you asked for.
- **Your key stays on your computer**, in the system keychain. It is handled only by the Rust core; the web-based interface cannot read it.
- **Your chats stay on your computer**, in a local database in the app's data folder.
- **Your messages go to OpenRouter** and the model provider you choose, under [OpenRouter's privacy terms](https://openrouter.ai/privacy).
- **In a project chat, what the tools return also goes to them.** Everything the tools return (file contents, folder listings and file names, search results and command output) is sent to OpenRouter and the model provider you chose, along with the name of the project folder and the conversation itself. Your key is sent only to OpenRouter, as the credential for requests. Beluga contacts no other server (GitHub only for skills, as above). Commands you approve can use the network on their own; that is separate from the app. See [What it can and cannot protect you from](#what-it-can-and-cannot-protect-you-from).
- The app's content security policy blocks all remote scripts, styles and fonts.

Found a security problem? Please report it privately through [GitHub security advisories](https://github.com/shamirul-007/beluga-desktop/security/advisories/new) rather than a public issue.

## Roadmap

Version 0.3 is the third of six planned stages.

| Stage | What it adds | Status |
|---|---|---|
| 1 | Sign-in, key storage, chat, model picker, gauge, appearance modes | **Released as 0.1** |
| 2 | Commands and skills: the `/` menu, `@` files, the Skills screen, bundled skill packs, message actions | **Released as 0.2** |
| 3 | Work on a project folder: read and edit files, run commands with your approval, spend limits | **In 0.3** |
| 4 | Agent skills that act on their own: planning, test-first development, debugging, code review and design guidance | Planned |
| 5 | Skill store with verified packs, and tested "Recommended" models | Planned |
| 6 | Signed installers and automatic updates | Planned |

Also planned, with no date: a tested "Recommended" list of models, and an operating-system sandbox for commands (there is none today, and there is no usable native sandbox on Windows yet).

The full design is in [`docs/superpowers/specs`](docs/superpowers/specs/2026-11-11-beluga-desktop-design.md); project work has its own design in [`2026-11-12-beluga-project-work-design.md`](docs/superpowers/specs/2026-11-12-beluga-project-work-design.md).

## Bundled skill packs

Beluga ships these skill packs, copied unchanged from their authors' repositories at a fixed commit, and credits each below. Skills are plain instruction files: Beluga never runs anything inside a pack. The packs and their licenses are listed in [`skills.lock.json`](skills.lock.json).

| Pack | Repository | License |
|---|---|---|
| Superpowers | [obra/superpowers](https://github.com/obra/superpowers) | MIT |
| Agent Skills | [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) | MIT |
| Anthropic Skills | [anthropics/skills](https://github.com/anthropics/skills) | Apache-2.0 |
| Emil Kowalski | [emilkowalski/skills](https://github.com/emilkowalski/skills) | MIT |
| Engineer Skills | [Jeffallan/claude-skills](https://github.com/Jeffallan/claude-skills) | MIT |
| gstack | [garrytan/gstack](https://github.com/garrytan/gstack) | MIT |
| ECC | [affaan-m/ECC](https://github.com/affaan-m/ECC) | MIT |
| Matt Pocock | [mattpocock/skills](https://github.com/mattpocock/skills) | MIT |
| Impeccable | [pbakaus/impeccable](https://github.com/pbakaus/impeccable) | Apache-2.0 |
| UI UX Pro Max | [nextlevelbuilder/ui-ux-pro-max-skill](https://github.com/nextlevelbuilder/ui-ux-pro-max-skill) | MIT |
| Expo | [expo/skills](https://github.com/expo/skills) | MIT |
| React Native | [callstackincubator/agent-skills](https://github.com/callstackincubator/agent-skills) | MIT |
| Apollo GraphQL | [apollographql/skills](https://github.com/apollographql/skills) | MIT |
| Astral | [astral-sh/agent-plugins](https://github.com/astral-sh/agent-plugins) | Apache-2.0 and MIT |
| Agents | [wshobson/agents](https://github.com/wshobson/agents) | MIT |

Three more packs are optional. They are not in the installer; you can install them from the Skills screen, which shows their license and any warning first:

| Pack | Repository | License |
|---|---|---|
| Trail of Bits | [trailofbits/skills](https://github.com/trailofbits/skills) | CC-BY-SA-4.0 |
| Vercel | [vercel-labs/agent-skills](https://github.com/vercel-labs/agent-skills) | No license file in the repository |
| Claude-Red | [SnailSploit/Claude-Red](https://github.com/SnailSploit/Claude-Red) | MIT |

Each pack keeps its own license file inside the app. **Settings → About → Open-source licenses** shows them.

## Build from source

**You need:** [Rust](https://rustup.rs) (stable), [Node.js](https://nodejs.org) 22 or later, [pnpm](https://pnpm.io) 10, and the [Tauri system prerequisites](https://tauri.app/start/prerequisites/) for your platform.

```bash
git clone https://github.com/shamirul-007/beluga-desktop.git
cd beluga-desktop
pnpm install

pnpm tauri dev      # run the app with live reload
pnpm tauri build    # build an installer for your system
```

Run the checks the CI runs:

```bash
pnpm test                                             # interface tests
pnpm exec tsc --noEmit                                # type check
cargo test   --manifest-path src-tauri/Cargo.toml     # core tests
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
```

## How it is built

Beluga is a [Tauri 2](https://tauri.app) app: a Rust core with a React and TypeScript interface.

```
src-tauri/src/
  auth/         OpenRouter sign-in (PKCE) and the local callback listener
  secrets.rs    key storage: keychain, session-only, or private file
  openrouter/   API client: models, key status, streaming chat
  engine/       the boundary every chat turn runs through
  agent/        project work: tools, approvals, the command runner, guards
  store/        local SQLite database for chats and usage
  commands/     the typed commands the interface may call
src/
  lib/ipc.ts    the only bridge between interface and core
  features/     sign-in, shell, chat, models, gauge, settings, commands, mentions, skills
```

All network access, key handling and storage live in the Rust core. The interface can only call a fixed set of typed commands, which is what keeps the key out of the web layer.

## Contributing

Issues and pull requests are welcome. Please run the checks above before opening a pull request, and keep changes focused. Known gaps and deferred items are listed in [`docs/superpowers/plans`](docs/superpowers/plans/2026-11-11-beluga-plan-1-follow-ups.md).

## License

[MIT](LICENSE). Beluga is an independent project and is not affiliated with OpenRouter.
