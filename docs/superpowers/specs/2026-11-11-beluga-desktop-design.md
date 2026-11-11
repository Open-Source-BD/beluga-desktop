# Beluga: a free-to-start desktop coding assistant on OpenRouter

Design, 2026-11-11. "Beluga" is a placeholder name (taken from the project folder); the owner has not chosen a name.

Items marked **(owner)** were stated or approved by the product owner. Items marked **(my call)** were decided by the designer and can be overturned.

## Intent

Developers who cannot afford a premium AI subscription should still be able to work the way a professional does. Beluga is a desktop app they install, connect to their own OpenRouter account, and use for free (on free models) or for cents (on cheap ones). It ships with the skill packs that make an AI agent plan before coding, test what it writes, review its work and check for security problems, so a beginner on a weak model still gets a disciplined workflow. **(owner)**

## Who it is for

Developers with little or no budget, from beginners upward. They watch every request and every cent. **(owner)**

## Goals

1. Cross-platform desktop app: macOS, Windows, Linux. Rust-based. **(owner)**
2. On first open, a sign-in popup that sends the user to OpenRouter and comes back with a working key. **(owner)**
3. After sign-in it works like a familiar chat assistant, and can also work on a code project on the user's computer: read files, edit them, run commands. **(owner)**
4. Professional-workflow skills are pre-installed and work with no setup. **(owner)**
5. A skill store in the first version, so the packs the owner named (superpowers, gstack, ECC, claude-red) are all available from day one. **(owner)**
6. Light, dark and system appearance modes. **(owner)**
7. The user always sees what they have left and what they have spent. **(my call)**

## Non-goals (first version)

- Accounts, servers or telemetry of our own. Everything lives on the user's computer. **(my call)**
- Providers other than OpenRouter. **(my call, follows from owner's "using this provider")**
- Mobile apps. **(my call)**
- Plugins that connect outside tools (MCP servers), cross-chat memory, parallel sub-agents, auto-update, sync between computers. All later. **(my call, owner approved the list)**

## Anti-goals

- Looking or being named like Anthropic's Claude desktop app. The layout family is the same (sidebar, conversation, message box); the name, palette, type and icon are our own. **(my call)**
- Running a command or changing a file outside the chosen project folder without the user saying yes. **(my call)**
- Hiding cost. A free-tier user must never be surprised by an exhausted daily limit or a charge. **(my call)**
- Shipping offensive-security skills switched on by default. **(my call)**
- Hard-coding model names. The free roster changes too often.

## Constraints and facts (researched 2026-11-11)

**OpenRouter**
- OAuth PKCE: open `https://openrouter.ai/auth?callback_url=…&code_challenge=…&code_challenge_method=S256`; exchange at `POST https://openrouter.ai/api/v1/auth/keys` with `{code, code_verifier, code_challenge_method}`; response contains `key`. Codes expire in 10 minutes. Localhost callbacks on any port are supported. Omitting `callback_url` gives a paste-a-code mode.
- Free models (`:free`): 20 requests/minute; 50 requests/day for accounts that have bought fewer than 10 credits ever, 1000/day otherwise.
- `GET /api/v1/models` returns pricing and `supported_parameters` (filter on `tools`). `GET /api/v1/key` returns usage, `is_free_tier` and `free_model_daily_requests {used, limit, remaining}`.
- Twelve free models existed on the research date, all advertising tool support. Advertised is not reliable; none were tested.

**Skills**
- Open spec at agentskills.io. A skill is a folder with `SKILL.md` (YAML frontmatter: `name`, `description` required) plus optional `scripts/`, `references/`, `assets/`. Progressive disclosure: name+description catalog at start, full body on activation, bundled files on demand.

**Tauri 2**
- No official OS-keychain plugin. Use the `keyring` crate from Rust so the key never reaches the web view. Linux needs a Secret Service provider; a fallback is required.
- OAuth callback: temporary localhost listener (e.g. `tauri-plugin-oauth`), not deep links.
- Known Linux pain: WebKitGTK blank windows on some NVIDIA setups (`WEBKIT_DISABLE_DMABUF_RENDERER=1`).

## How it gets made

| Choice | Decision | Whose |
|---|---|---|
| Language/platform | Rust-based | owner |
| App shell | Tauri 2: Rust core, web-technology interface | my call |
| Interface stack | React + TypeScript. assistant-ui (the owner's reference) may be used for chat primitives if its license and Tauri fit check out | my call |
| Agent engine | Embed goose (Apache-2.0, Rust, supports OpenRouter, skills, MCP) as a separate process talking over ACP; our app owns all UI | my call; owner was asked twice and did not object |
| Engine fallback | If goose proves unworkable: a small in-house loop (OpenAI-compatible client pointed at OpenRouter + our own skills loader). The interface must talk to the engine through one internal boundary so this swap is possible | my call |
| Key storage | OS keychain via `keyring`; encrypted-file fallback on Linux without Secret Service | my call |
| Local data | Chats, settings and installed skills in the app's data folder on the user's computer | my call |

The engine choice carries the most risk: goose's internals were changing on the research date (moving to ACP, phasing out its REST server). The first piece of building should be a short spike proving goose can be driven from a Tauri app with an OpenRouter key, a skill loaded, and a permission prompt surfaced to our UI. If the spike fails, use the fallback.

## What the user sees

Approved mockup (all three modes): `.superpowers/brainstorm/71836-1794373193/content/design-v3.html`. **(owner approved: "looks perfect")**

**Look.** Cold-water palette. Light: window `#F4F7F9`, sidebar `#E9EEF2`, hairlines `#D5DEE5`, ink `#0F2B3C`, secondary `#56707F`, accent teal `#0E7C8B`. Dark: window `#0F1C24`, sidebar `#0A151B`, ink `#E3ECF1`, accent `#45B9C7`. Amber is used only where the app needs a decision. Headings in Bricolage Grotesque, body in Hanken Grotesk, code in JetBrains Mono. Fonts are bundled, not fetched. No emoji as icons. Full token list is in the mockup's CSS.

**Appearance.** Settings has System (default), Light, Dark. System follows the OS and changes live.

**First open.** Split panel. Left: brand panel (stays navy in both modes) with the line "A senior developer's habits, on a free model." Right: "Connect your OpenRouter account", a primary "Sign in with OpenRouter" button, a secondary "Paste a key I already have", and a note that the key stays in the computer's keychain.

**Main window.** Sidebar: New chat, Projects (folders), earlier chats, Skills, Settings, and the free-request gauge at the bottom. Main area: a header with chat title, project name and this chat's request count and cost; the conversation; the message box.

**New chat.** A large greeting ("What are we building?"), a line saying which project is open and what the app may do there, the message box, and three starter actions (plan a feature, find and fix a bug, review recent changes).

**Message box.** Holds Attach, the model picker, and a Skills (`/`) menu, plus Send.

**Mid-task.** The agent's replies are plain text. Which skill is in use shows as a small teal line. Files read and written show as a quiet indented trail. A request to run a command shows as an amber card with the exact command and three choices: Run once, Always allow this kind of command in this project, Don't run.

**Free-request gauge.** A number ("38 of 50 free requests left"), a row of marks with one spent per request, the reset time, and money spent this month. For accounts on the 1000/day tier the marks scale (builder's choice how). For a user on a paid model the gauge shows spend and credit balance in place of the marks.

## Behaviour

**Sign-in.** PKCE with a localhost callback. If the browser cannot return to the app, offer the paste-a-code path. Pasted keys are validated with `GET /api/v1/key` before being saved. Sign out removes the key from the keychain.

**Models.** Fetched live and cached briefly. The picker groups Free and Paid, shows price per million tokens for paid ones, and lists only models that support tools when a project is open. Default model: the free tool-capable model with the best tool-calling reliability OpenRouter reports; if that signal is unavailable, the builder picks a rule and documents it.

**Chat.** Streaming replies, stop button, history saved locally, file attachments (text and images where the model accepts them), rename and delete chats.

**Project work.** The user opens a folder. Inside it the agent may read files freely, edit files (edits shown in the trail; builder decides whether edits need per-edit approval or are reviewable after, default: allowed inside the project, shown as they happen), and run commands only after approval. "Always allow" rules are stored per project and listed in Settings where they can be removed. Nothing outside the project folder is touched without an explicit prompt.

**Running low.** At 10 free requests left the gauge turns amber. At 0 the app says when the limit resets and offers the cheapest paid tool-capable models with their prices. It never switches to a paid model by itself.

**Errors.** Rate limit, no credit, model unavailable, and network loss each get a plain message saying what happened and what to do next, with a retry.

## Skills

**Loading.** Follow the agentskills.io guide: catalog of name+description in the system prompt, activate on demand, lenient YAML parsing, skill folders readable by the agent, skill content protected from context compaction. superpowers needs its `using-superpowers` starter skill injected at the start of every session and again after compaction, with a tool-name mapping for our engine (superpowers ships per-harness mapping files to copy from). Where a skill expects sub-agents, the first version falls back to the skill's own sequential path (`executing-plans`).

**Tier 1, pre-installed and on**

| Pack | License |
|---|---|
| `obra/superpowers` | MIT |
| `addyosmani/agent-skills` | MIT |
| Anthropic `frontend-design` (from `anthropics/skills`) | Apache-2.0, per skill |
| `emilkowalski/skills` | MIT |
| `vercel-labs/agent-skills` (React, React Native, web design rules; not the deploy skills) | MIT stated in README and frontmatter only, no LICENSE file |
| `Jeffallan/claude-skills` | MIT |

**Tier 2, one click in the store**

- `garrytan/gstack` (MIT per GitHub; LICENSE text not read) and `affaan-m/ECC` (MIT): install the skill files only. Their hooks, agents, installers and MCP configs are Claude Code-specific and are skipped in the first version. ECC: use the 314 canonical skills under `skills/`, not the per-harness copies.
- `mattpocock/skills` (MIT), `pbakaus/impeccable` (Apache-2.0, keep NOTICE), `nextlevelbuilder/ui-ux-pro-max-skill` (MIT, needs Python).
- `trailofbits/skills` (CC-BY-SA-4.0): shipped unmodified, as its own pack, with attribution. Never merged into our own prompts. Several skills need Semgrep or CodeQL installed; the store says so.
- Stack packs: `expo/skills`, `callstackincubator/agent-skills`, `apollographql/skills`, `astral-sh/agent-plugins`, Next.js official skills.

**Tier 3, opt-in with a warning**

- `SnailSploit/Claude-Red` (MIT): offensive security. Off by default, installed only after the user reads and accepts a notice that it is for authorised testing only.

**Not included:** `semgrep/skills` (source-available license), aggregator lists with missing or non-commercial licenses, `claude-mem` (not portable).

**Store.** A "Verified" tab is our own allowlist of the packs above, each pinned to a commit. A search tab may use the skills.sh search feed for discovery; its terms of use and rate limits were not checked and must be before shipping. Every listing shows source repo, license (read from the skill itself, not just the repo) and what extra tools it needs. Installed skills can be turned off or removed. Skills that carry scripts show that they do before install.

**Licensing rules for the builder.** Pin each bundled pack to a commit. Ship each pack's LICENSE and NOTICE files with it. Add an in-app "Open-source licenses" screen.

## Decisions added after builder review (all my call)

**Who owns the skills loader.** We do, in our own Rust code, whichever engine runs underneath. The loader builds the catalog, handles on/off, activation, the superpowers starter injection and re-injection after compaction, and passes the result to the engine as extra system instructions. goose's built-in skills feature is not relied on. This is why the loader has automated tests of its own.

**Spike pass/fail.** Stock goose passes if, driven from a Tauri app, it (1) runs a tool loop on an OpenRouter key we supply, (2) accepts system instructions we inject per session, (3) hands every command-run request to our UI and waits for our answer, and (4) can be stopped mid-task and continued later from saved conversation. If any of the four needs a fork of goose, the spike has failed and the in-house loop is built. We do not maintain a goose fork.

**How skills reach the user.** Tier 1 packs that have a LICENSE file are bundled in the installer, so the app works with no network beyond OpenRouter. `vercel-labs/agent-skills` has no LICENSE file, so it is fetched from GitHub at its pinned commit on first run rather than redistributed. Tier 2 and 3 packs are fetched from GitHub at pinned commits when the user installs them, and checked against a recorded checksum.

**Where the Verified list lives.** A manifest file in a public GitHub repository of ours (pack, repo, pinned commit, checksum, license, required tools, tier). The app fetches it when the store opens and falls back to the copy baked into the installer. This is a static file, not a server of our own, and lets a pack be corrected or pulled without a new release.

**Curation of Tier 1.** "Pre-installed and on" does not mean every skill in all six packs. All six packs are installed; a curated set of at most about 40 skills is switched on by default, chosen to avoid duplicates (one planning skill, one TDD skill, one debugging skill, one review skill, and so on, with superpowers preferred where packs overlap). The rest are one toggle away in the Skills screen. Language and framework skills switch on when a matching project is opened (for example a `Cargo.toml` turns on the Rust skill). The catalog sent to the model must stay under roughly 4,000 tokens.

**"Always allow this kind of command."** The rule matches the program name plus its first sub-command (`npm install`, `git status`, `cargo test`), in that project only. A command containing a pipe, `&&`, `;`, a redirect, a sub-shell or `sudo` never matches a saved rule and always prompts. There is no OS-level sandbox in the first version: the approval prompt is the safety boundary, and the prompt says so plainly when a command looks like it writes outside the project. Scripts that ship inside a skill prompt like any other command and show the full path.

**Spend guards.** Every task pauses and asks "Keep going?" after 25 requests, showing how many free requests remain or what has been spent. At 0 free requests the task pauses with its conversation saved, and continues when the user returns or picks another model. For paid models there is a daily spend limit, default $1.00, changeable in Settings; reaching it pauses the task the same way.

**Linux without a keychain service.** No silent weak encryption. The app explains the situation and offers two choices: keep the key for this session only (default), or save it in a file readable only by the user's account, with a plain warning that this is less protected than a keychain.

## Left to the builder

- Internal structure of the Rust core and the web interface.
- Local storage format for chats and settings.
- Exact packaging and signing per platform (unsigned builds are acceptable for early testing).
- How the gauge scales for the 1000/day tier.
- Whether assistant-ui is used or the chat components are written by hand.
- Test strategy, with one fixed requirement: sign-in, key storage, the permission prompt and the skills loader must have automated tests.

## Owner decisions after builder review

**Free models: be honest and guide up. (owner)** Early in the build, a small test harness runs a fixed multi-step task (read files, edit, run a command, read the result, finish) against the tool-capable free models and the cheapest paid ones. Models that pass are marked "Recommended" in the picker; the list of passing models ships in the Verified manifest so it can be updated without a release. Free remains the default. The app says plainly, at sign-in and when the gauge runs low, that a one-time $10 OpenRouter top-up raises the free limit from 50 to 1,000 requests a day, and it suggests the cheapest Recommended paid models when a free one keeps failing. It never switches model by itself. If no free model passes the test, the app still ships: free models are offered for chat and small tasks, and project work shows a notice recommending a tested model.

**Windows from day one. (owner)** macOS, Windows and Linux ship together with equal support, including project work. The plan must include, from the first milestone: a Windows shell strategy (builder's choice between PowerShell and a detected Git Bash, documented), command-rule matching that works for both shell families, and a per-skill compatibility flag in the manifest so skills that need a Unix shell or extra tools say so on Windows rather than failing.

**Open source under MIT. (owner)**

**Name: decide later. (owner)** "Beluga" is a working name held in one place in the code. The bundle identifier and keychain service name must be fixed before any public build, so the owner is asked for the final name before the first public release.
