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

> **Early preview.** This is version 0.2: the chat foundation plus commands and skills. It is tested by an automated suite on macOS, Windows and Linux, but it is young software: expect rough edges and please [report what you find](https://github.com/shamirul-007/beluga-desktop/issues). "Beluga" is a working name and may change.

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
| **Skills screen** | Turn skills and whole packs on or off, read what a skill does, add your own from a folder, a zip or a GitHub address, and install optional extra packs. |
| **Message actions** | Copy any message, copy a code block, retry a reply (optionally with another model), and edit and resend a message. Up arrow in an empty box edits your last message; a message sent while a reply is running waits in line; Esc stops a reply. |
| **Bundled skill packs** | Fifteen skill packs for planning, test-first work, debugging, review and design are included; 21 skills are on by default. See [Bundled skill packs](#bundled-skill-packs). |
| **Light, dark and system modes** | Follows your operating system by default. |

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
- The app's content security policy blocks all remote scripts, styles and fonts.

Found a security problem? Please report it privately through [GitHub security advisories](https://github.com/shamirul-007/beluga-desktop/security/advisories/new) rather than a public issue.

## Roadmap

Version 0.2 is the second of six planned stages.

| Stage | What it adds | Status |
|---|---|---|
| 1 | Sign-in, key storage, chat, model picker, gauge, appearance modes | **Released as 0.1** |
| 2 | Commands and skills: the `/` menu, `@` files, the Skills screen, bundled skill packs, message actions | **Released as 0.2** |
| 3 | Work on a project folder: read and edit files, run commands with your approval, spend limits | Planned |
| 4 | Agent skills that act on their own: planning, test-first development, debugging, code review and design guidance | Planned |
| 5 | Skill store with verified packs, and tested "Recommended" models | Planned |
| 6 | Signed installers and automatic updates | Planned |

The full design is in [`docs/superpowers/specs`](docs/superpowers/specs/2026-11-11-beluga-desktop-design.md).

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
