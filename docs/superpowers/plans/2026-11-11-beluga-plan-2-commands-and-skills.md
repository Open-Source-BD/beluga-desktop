# Beluga Plan 2: Commands, Mentions and Skills Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Typing `/` runs commands and skills, typing `@` attaches files from an opened folder, a Skills screen shows and manages everything installed, and fifteen skill packs ship inside the installer.

**Architecture:** A new `skills` module in the Rust core owns everything about skills: one lenient parser, one text scanner, one pack copier and one GitHub archive fetcher, used both by a developer tool that vendors packs into `src-tauri/resources/skills/` and by the running app when a user installs or adds skills. Skills are chat state: active skill bodies are placed ahead of the conversation when a turn's wire messages are built. The interface gains one popover menu component used by both `/` and `@`.

**Tech Stack:** As Plan 1, plus Rust crates `flate2`, `tar`, `zip`, `ignore`, `sha2` (present), and `tauri-plugin-dialog`.

**Spec:** `docs/superpowers/specs/2026-11-11-beluga-commands-and-skills-design.md`. Mockup: `docs/superpowers/mockups/commands-and-skills.html`. Pack inventory: `docs/superpowers/research/2026-11-11-skill-pack-inventory.json`. Known gaps carried from Plan 1: `docs/superpowers/plans/2026-11-11-beluga-plan-1-follow-ups.md`.

## Global Constraints

- All Plan 1 constraints still hold: the key never reaches the interface; no telemetry; no hard-coded model ids outside fixtures; the working name only in `brand.rs`, `brand.ts` and `productName`; bundled fonts; inline SVG icons; the approved colour tokens; `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `tsc --noEmit` clean; user-facing copy quoted in this plan is exact.
- Nothing inside a skill pack is ever executed, by the app, the vendoring tool, or a test.
- Vendored pack files are copied byte-for-byte and never edited.
- Only these file types are copied from a skill folder: `.md`, `.txt`, `.json`, `.yaml`, `.yml`, `.csv`, each at most 256,000 bytes. Symbolic links are skipped. A file whose path inside the vendored tree exceeds 180 characters is skipped.
- The vendored tree is at most 25 MB on disk and adds at most 8 MB to any installer.
- At most 40 skills are on by default.
- New network hosts, contacted only when the user presses Install or Add: `github.com`, `codeload.github.com`, `api.github.com`. The web view's CSP is unchanged; these requests are made by the Rust core.
- File reading for `@` goes through the Rust core and never leaves the chosen folder.
- CI exists now: every task's commits must pass the CI workflow on macOS, Windows and Linux. The controller pushes and checks.
- Existing 0.1.x databases are upgraded in place and keep their chats.

## Review Focus

1. A skill pack containing a hostile archive entry (`../` paths, absolute paths, symbolic links, a 2 GB file). Extraction must reject or skip it and never write outside its staging folder. Tests in Tasks 3 and 10.
2. A `SKILL.md` with no frontmatter, broken YAML, a UTF-8 byte-order mark, Windows line endings, or a 5 MB body. It must load as a listed skill or a listed broken skill, never crash the list. Tests in Task 1.
3. A folder for `@` that contains a link pointing outside it, 100,000 files, or unreadable entries. Listing stays inside, stops at the cap, and skips what it cannot read. Tests in Task 9.
4. A 0.1.x database with existing chats. After upgrade every chat and message is still there and sendable. Test in Task 5.
5. A skill active in a chat when the model is switched to one with a small context. The skill is marked too long and not sent, and the chat still works. Tests in Tasks 7 and 11.

---

## File Structure

```
skills.lock.json                         every pack: repo, commit, selection rule, license, checksum
src-tauri/
  resources/skills/<pack-id>/            vendored text files + pack.json + license files
  resources/skills/default-skills.json   ids switched on by default
  resources/skills/acts-overrides.json   hand-set acts-on-your-computer marks
  src/bin/vendor_skills.rs               developer tool: regenerate the vendored tree from the lock
  src/skills/
    mod.rs        shared types
    parse.rs      SKILL.md -> name, description, body
    scan.rs       acts-on-your-computer text scan
    pack.rs       select skill folders by rule, copy text files, build PackManifest
    archive.rs    download and safely extract a GitHub archive or a ZIP
    library.rs    everything installed: built-in, installed, added by you; switches
    install.rs    add from folder / ZIP / GitHub link; install and remove extra packs
  src/store/migrate.rs                   schema versions
  src/store/chat_state.rs                active skills, folder, summary, boundary
  src/folder.rs                          list and read files inside a chat's folder
  src/commands/{skills,chat_state,folder}.rs
src/
  features/menu/      PopMenu.tsx, match.ts          shared popover list and name matching
  features/commands/  registry.ts, useSlashMenu.ts, Help.tsx, Usage.tsx
  features/mentions/  useMentionMenu.ts
  features/skills/    SkillsScreen.tsx, PackCard.tsx, SkillDetail.tsx, AddSkill.tsx, MorePacks.tsx, RedWarning.tsx, ActiveSkills.tsx
  features/chat/      (modified) Composer, Thread, useChat; new Fullness.tsx, Queued.tsx
```

Shared types (defined in the task named; later tasks use them as written):

```rust
// skills/mod.rs (Task 1)
pub struct ParsedSkill { pub name: String, pub description: String, pub body: String, pub license: Option<String> }
pub struct SkillEntry { pub id: String, pub pack: String, pub name: String, pub description: String,
    pub dir: String, pub acts_on_computer: bool, pub body_chars: u32, pub files: Vec<String>,
    pub license: Option<String>, pub broken: Option<String> }
// Task 2
pub struct PackSpec { pub id: String, pub title: String, pub repo: String, pub commit: String,
    pub select: Vec<String>, pub exclude: Vec<String>, pub license: String, pub bundled: bool,
    pub about: String, pub checksum: String }
pub struct PackManifest { pub id: String, pub title: String, pub repo: String, pub commit: String,
    pub license: String, pub license_files: Vec<String>, pub skills: Vec<SkillEntry>,
    pub skipped: Vec<String>, pub checksum: String }
// Task 6
pub enum PackOrigin { BuiltIn, Installed, Yours }   // serialises "built_in" | "installed" | "yours"
```

`SkillEntry.id` is `"{pack}:{name}"`, or `name` alone when `pack == "yours"`. `dir` is relative to the pack root; `files` are relative to `dir` and exclude `SKILL.md`.

---

### Task 1: Skill parsing and the text scan

**Files:**
- Create: `src-tauri/src/skills/{mod.rs,parse.rs,scan.rs}`, fixtures under `src-tauri/tests/fixtures/skills/`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `ParsedSkill`, `SkillEntry` (above); `pub fn parse_skill(text: &str, dir_name: &str) -> Result<ParsedSkill, String>`; `pub fn acts_on_computer(body: &str) -> bool`; `pub const MAX_BODY_CHARS: usize = 60_000`.

Parsing rules: strip a UTF-8 BOM and normalise `\r\n`. If the text starts with `---`, the block up to the next `---` line is frontmatter; read `name`, `description` and `license` leniently (plain `key: value` lines, quoted or not, and `|`/`>` block scalars; an unparseable line is ignored, not fatal). `name` falls back to `dir_name`. `description` falls back to: the first non-empty line under a heading containing "Description" or "Metadata" (skipping list markers and a leading `**Description:**`-style label), else the first paragraph that is not a heading. Descriptions are collapsed to one line and cut at 200 characters on a word boundary. `Err` only when there is no body text at all.

Scan rule: `true` when the body, outside frontmatter, matches any of: a fenced block tagged `bash`, `sh`, `shell`, `zsh`, `powershell` or `console`; a tool name as a whole word (`Bash`, `Read`, `Edit`, `Write`, `Task`, `TodoWrite`, `WebFetch`, `mcp__`); the phrases "run the command", "run this command", "dispatch a subagent", "spawn a subagent", "create the file", "write the file", "edit the file", "git commit", "git worktree", case-insensitive.

- [ ] **Step 1: Write the failing tests.**

```rust
// parse: frontmatter_name_and_description
// parse: quoted_values_and_block_scalar_description
// parse: missing_frontmatter_uses_dir_name_and_first_paragraph   (fixture copied from a claude-red style "# SKILL / ## Metadata" file)
// parse: name_in_frontmatter_differs_from_dir -> frontmatter wins
// parse: broken_yaml_line_is_ignored_and_other_fields_still_read
// parse: bom_and_crlf_are_tolerated
// parse: description_is_one_line_and_at_most_200_chars
// parse: empty_file_is_err; frontmatter_only_is_err
// parse: five_megabyte_body_parses_without_panic
// parse: license_field_is_read
// scan: bash_fence_is_true; plain_prose_is_false; tool_name_as_whole_word_is_true ("Reading" alone is false)
// scan: phrases_are_case_insensitive
```

- [ ] **Step 2:** Run `cargo test --manifest-path src-tauri/Cargo.toml skills::`. Expected: fail to compile.
- [ ] **Step 3:** Implement. No YAML crate: the lenient line reader above is the whole parser.
- [ ] **Step 4:** Run the tests. Expected: pass. Commit: `feat(skills): lenient SKILL.md parser and acts-on-computer scan`.

### Task 2: Selecting and copying a pack

**Files:**
- Create: `src-tauri/src/skills/pack.rs`

**Interfaces:**
- Consumes: `parse_skill`, `acts_on_computer`, `SkillEntry`.
- Produces: `PackSpec`, `PackManifest` (above);
  `pub fn select_skill_dirs(root: &Path, select: &[String], exclude: &[String]) -> Vec<PathBuf>`;
  `pub fn copy_pack(src_root: &Path, spec: &PackSpec, dest: &Path, overrides: &HashMap<String, bool>) -> Result<PackManifest, String>`;
  `pub fn tree_checksum(dir: &Path) -> String` (SHA-256 over sorted relative paths and file bytes, hex);
  `pub fn read_manifest(pack_dir: &Path) -> Result<PackManifest, String>` (reads `pack.json`).

Selection: each `select` pattern is a `/`-separated path where `*` matches exactly one segment and `.` means the root itself; a match counts when that folder contains `SKILL.md`. A folder whose relative path starts with any `exclude` prefix is dropped. Results are sorted.

Copy: for each selected folder, parse `SKILL.md` (a parse failure produces an entry with `broken: Some(reason)`), then copy allowed files (Global Constraints) preserving relative paths under `dest/<dir>`. The root pattern `.` copies only that folder's own `SKILL.md`, not the tree. License files matching `LICENSE*`, `NOTICE*`, `THIRD_PARTY_NOTICES*` at the source root, and a `licenses/` folder if present, are copied to `dest/`. Skill names repeated inside one pack: the later one takes its folder name. `overrides` maps skill id to the acts mark and wins over the scan. Skipped links, oversized files and over-long paths go to `skipped`. The manifest is written to `dest/pack.json`; `checksum` is `tree_checksum(dest)` computed before `pack.json` is written.

- [ ] **Step 1: Write the failing tests** (build source trees in temp dirs):

```rust
// select: "skills/*" finds only folders with SKILL.md; "plugins/*/skills/*" two levels; "Skills/*/*"
// select: "." plus "*" finds the root skill and depth-1 skills; exclude ["test", "openclaw"] removes them
// copy: copies .md/.txt/.json/.yaml/.yml/.csv and skips .sh/.py/.png/.ttf
// copy: a 300 KB .json is skipped and listed in skipped
// copy: a symlink is skipped and listed (cfg(unix))
// copy: a path longer than 180 chars is skipped and listed
// copy: copied bytes are identical to the source bytes (CRLF file stays CRLF)
// copy: LICENSE, NOTICE.md and licenses/ are copied to the pack root
// copy: a skill with no usable SKILL.md is in the manifest with broken set, and the pack still copies
// copy: duplicate names inside a pack -> second uses its folder name
// copy: ids are "pack:name"; pack "yours" gives bare names
// copy: overrides win over the scan
// checksum: same tree -> same value; one changed byte -> different
// read_manifest roundtrips what copy_pack wrote
```

- [ ] **Step 2:** Run `cargo test skills::pack`. Expected: fail to compile.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: pass. Commit: `feat(skills): select skill folders and copy a pack's text files`.

### Task 3: Fetching and unpacking archives safely

**Files:**
- Create: `src-tauri/src/skills/archive.rs`
- Modify: `src-tauri/Cargo.toml` (`flate2`, `tar`, `zip`, `tempfile` as a regular dependency)

**Interfaces:**
- Produces:
  `pub struct Limits { pub max_total_bytes: u64, pub max_file_bytes: u64, pub max_entries: usize }` with `Limits::default()` = 300 MB, 50 MB, 50,000;
  `pub async fn download_github_archive(http: &reqwest::Client, codeload_base: &str, repo: &str, commit: &str, dest: &Path, limits: &Limits) -> Result<PathBuf, String>` (GET `{codeload_base}/{repo}/tar.gz/{commit}`; returns the single top-level folder inside `dest`);
  `pub fn unpack_tar_gz(reader: impl Read, dest: &Path, limits: &Limits) -> Result<(), String>`;
  `pub fn unpack_zip(path: &Path, dest: &Path, limits: &Limits) -> Result<(), String>`.
  Production `codeload_base` is `https://codeload.github.com`.

Unpacking rules: only regular files and folders are written; links, devices and other entry types are skipped. An entry whose path is absolute, contains `..`, or resolves outside `dest` aborts the whole unpack with an error. Sizes are enforced while streaming (declared sizes are not trusted). Nothing is made executable.

- [ ] **Step 1: Write the failing tests** (build archives in memory with the `tar`/`zip` crates):

```rust
// tar: a normal archive unpacks to the same bytes
// tar: an entry named "../evil" -> Err, and nothing exists outside dest
// tar: an absolute path entry -> Err
// tar: a symlink entry is skipped, other files still unpack
// tar: total size over the limit -> Err before finishing; a single file over max_file_bytes -> Err
// tar: more than max_entries -> Err
// zip: the same five cases
// download: wiremock serves a tar.gz at /owner/repo/tar.gz/abc123 -> returns dest/<top folder>
// download: 404 -> Err containing "not found"; a body larger than max_total_bytes -> Err
```

- [ ] **Step 2:** Run `cargo test skills::archive`. Expected: fail to compile.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: pass. Commit: `feat(skills): safe archive download and unpack`.

### Task 4: Vendor the fifteen packs

**Files:**
- Create: `skills.lock.json`, `src-tauri/src/bin/vendor_skills.rs`, `src-tauri/resources/skills/**`, `src-tauri/resources/skills/default-skills.json`, `src-tauri/resources/skills/acts-overrides.json`, `src-tauri/tests/vendored.rs`
- Modify: `src-tauri/tauri.conf.json` (`bundle.resources`: `"resources/skills/**/*"`), `src-tauri/Cargo.toml` (`[[bin]]`), `.gitattributes` (`src-tauri/resources/skills/** -text` so no line endings are rewritten)

**Interfaces:**
- Consumes: `PackSpec`, `copy_pack`, `download_github_archive`, `tree_checksum`, `read_manifest`.
- Produces: the vendored tree; `pub fn load_lock(text: &str) -> Result<Vec<PackSpec>, String>` in `skills/pack.rs`; the binary `cargo run --manifest-path src-tauri/Cargo.toml --bin vendor-skills -- [--check]`.

`skills.lock.json` holds all eighteen packs with exactly the ids, repositories, commits (full SHAs from the inventory file), selection rules and licenses in the spec's Facts table; `bundled: true` for the fifteen and `false` for `claude-red`, `vercel`, `trailofbits`; `about` is one sentence shown in the Skills screen. Selection rules as patterns:

| Pack | select | exclude |
|---|---|---|
| superpowers, agent-skills, emil, engineer, ecc, react-native, apollo, vercel | `skills/*` | |
| anthropic | `skills/*` | `template` |
| gstack | `.`, `*` | `openclaw`, `qm`, `capy`, `browser-skills`, `test` |
| mattpocock | `skills/*/*` | `skills/in-progress` |
| impeccable | `plugin/skills/*` | |
| ui-ux-pro-max | `.claude/skills/*` | |
| expo, astral, agents, trailofbits | `plugins/*/skills/*` | `plugins/code-improver/tests` |
| claude-red | `Skills/*/*` | |

The tool: for each bundled pack, download at the pinned commit into a temp folder, `copy_pack` into `src-tauri/resources/skills/<id>/`, record the checksum in the lock. `--check` re-reads the vendored tree and fails if any checksum differs from the lock. It never runs anything from a pack.

- [ ] **Step 1: Write the failing tests** in `src-tauri/tests/vendored.rs` (they read the committed tree; no network):

```rust
// lock_has_18_packs_15_bundled_with_the_expected_ids
// every_bundled_pack_has_a_pack_json_whose_checksum_matches_the_lock
// every_bundled_pack_ships_at_least_one_license_file  (anthropic: per-skill LICENSE.txt files count)
// skill_counts_match_the_spec: superpowers 15, agent-skills 25, anthropic 19, emil 18, engineer 67, gstack 63, ecc 314, mattpocock 31, impeccable 1, ui-ux-pro-max 7, expo 24, react-native 9, apollo 15, astral 3, agents 185
// vendored_tree_is_at_most_25_megabytes
// no_vendored_file_has_a_disallowed_extension_or_exceeds_256000_bytes
// default_skills_are_at_most_40_exist_and_are_not_broken
// default_skills_include: superpowers:brainstorming, superpowers:writing-plans, superpowers:systematic-debugging, superpowers:test-driven-development, anthropic:frontend-design
// default_skills_exclude: superpowers:dispatching-parallel-agents, superpowers:subagent-driven-development, superpowers:using-git-worktrees
// every_default_skill_has_an_entry_in_acts_overrides
// no_skill_id_is_duplicated_across_all_packs
```

- [ ] **Step 2:** Run `cargo test --test vendored`. Expected: fail (no tree).
- [ ] **Step 3:** Write the lock file and the tool, then run it. If a count differs from the spec because a repository has changed at that commit, stop and report rather than editing the expectation.
- [ ] **Step 4:** Check the size ceiling. If over 25 MB, re-run with `.json` and `.csv` left out for the largest packs first, and record what was dropped in the commit message.
- [ ] **Step 5:** Choose the default-on list by the spec's rules (read each candidate's `SKILL.md`): all of `superpowers` except the three excluded above; `anthropic:frontend-design`; then one best skill each, from `agent-skills`, `emil`, `engineer`, `mattpocock`, for: writing a spec, debugging, code review, refactoring, git and pull requests, documentation, UI polish, animation, React, TypeScript, Python, Node, Rust. Where `superpowers` already covers a job, pick nothing more for it. Write the ids to `default-skills.json` and each one's hand-set mark to `acts-overrides.json`.
- [ ] **Step 6:** Re-run the tool so overrides are applied, then `--check`, then all tests. Build once with `pnpm tauri build` and record the installer size before and after in the report; it must not grow by more than 8 MB.
- [ ] **Step 7:** Commit: `feat(skills): vendor fifteen skill packs with licenses and defaults`.

### Task 5: Store upgrade and chat state

**Files:**
- Create: `src-tauri/src/store/{migrate.rs,chat_state.rs}`, fixture `src-tauri/tests/fixtures/db_v1.sql` (the exact version 1 schema plus two chats with messages and usage)
- Modify: `src-tauri/src/store/{mod.rs,chats.rs}`

**Interfaces:**
- Produces:
  - `migrate(conn)` run by `Store::open` and `Store::in_memory`: version 1 to 2 adds to `chats`: `active_skills TEXT NOT NULL DEFAULT '[]'`, `folder TEXT`, `summary TEXT`, `sent_from_seq INTEGER`; to `messages`: `kind TEXT NOT NULL DEFAULT 'message'`; new table `switches(id TEXT PRIMARY KEY, is_on INTEGER NOT NULL)` for skills and packs (pack rows use the id `pack/<id>`).
  - `StoredMessage` gains `pub kind: String` (`"message"` or `"note"`) and `pub seq: i64`.
  - `Store::append_note(chat_id, text) -> Result<StoredMessage>`
  - `Store::active_skills(chat_id) -> Result<Vec<String>>`, `Store::set_active_skills(chat_id, &[String])`
  - `Store::folder(chat_id) -> Result<Option<String>>`, `Store::set_folder(chat_id, Option<&str>)`
  - `Store::summary(chat_id) -> Result<Option<(String, i64)>>` (text, `sent_from_seq`), `Store::set_summary(chat_id, text, sent_from_seq)`
  - `Store::delete_from(chat_id, message_id) -> Result<()>` (that row and every later row of the chat)
  - `Store::sendable_messages(chat_id) -> Result<Vec<StoredMessage>>` (kind `message`, `seq >= sent_from_seq` when set)
  - `Store::switch(id) -> Result<Option<bool>>`, `Store::set_switch(id, bool)`

- [ ] **Step 1: Write the failing tests.**

```rust
// a_v1_database_upgrades_in_place: load db_v1.sql into a file, open with Store::open, both chats and all messages and usage are intact, user_version is 2
// upgraded_messages_have_kind_message; opening_twice_is_a_no_op
// a_new_database_is_created_at_version_2
// notes_are_listed_by_get_messages_but_not_by_sendable_messages
// active_skills_roundtrip_and_default_to_empty
// folder_roundtrip_and_clear
// summary_sets_the_boundary: messages before sent_from_seq are not sendable, later ones are
// delete_from_removes_the_message_and_everything_after_including_notes_and_keeps_earlier_rows
// delete_from_with_an_unknown_id_changes_nothing_and_errors
// switches_roundtrip_and_unknown_is_none
// deleting_a_chat_still_cascades
```

- [ ] **Step 2:** Run `cargo test store`. Expected: fail.
- [ ] **Step 3:** Implement. Existing callers that build history (`commands/turn.rs`) switch to `sendable_messages` in Task 7; until then `get_messages` keeps returning everything.
- [ ] **Step 4:** Run all Rust tests. Expected: pass. Commit: `feat(store): schema version 2 with chat state, notes and switches`.

### Task 6: The skill library and its commands

**Files:**
- Create: `src-tauri/src/skills/library.rs`, `src-tauri/src/commands/skills.rs`
- Modify: `src-tauri/src/state.rs` (resource dir path), `src-tauri/src/commands/{mod.rs,handlers.rs}`, `src-tauri/src/lib.rs`, `src/lib/ipc.ts` and its test

**Interfaces:**
- Consumes: `read_manifest`, `PackManifest`, `SkillEntry`, `Store::switch`.
- Produces (Rust):
  - `Library::load(builtin_dir: &Path, data_skills_dir: &Path) -> Library`; built-in packs from `builtin_dir/<id>/pack.json`, installed packs from `data_skills_dir/packs/<id>/pack.json`, the user's own from `data_skills_dir/yours/pack.json`. A pack folder without a readable `pack.json` is listed as a pack with zero skills and an error line, not dropped.
  - `Library::reload(&self)`; `Library::packs(&self, store) -> Vec<PackView>`; `Library::skill(&self, id) -> Option<(PackView, SkillEntry)>`; `Library::skill_body(&self, id) -> Result<String, UiError>`; `Library::skill_file(&self, id, file) -> Result<Vec<u8>, UiError>` (only files listed in the entry); `Library::is_on(&self, store, id) -> bool`.
  - On/off rule: a stored switch wins; otherwise built-in skills are on only if listed in `default-skills.json`; skills in `yours` default to on; skills in installed packs default to off. A skill is offered in the `/` menu only when it is on, its pack is on, and it is not broken.
- Produces (commands and `ipc.ts`):

| Command | Arguments | Returns |
|---|---|---|
| `list_packs` | | `Vec<PackView>` |
| `get_skill` | `id` | `SkillDetail` |
| `set_skill_on` | `id, on` | `()` |
| `set_pack_on` | `id, on` | `()` |
| `skill_file` | `id, file` | `AttachmentInput` (text, mime `text/plain`) |

  `PackView { id, title, origin, license, repo, commit, about, on, error: Option<String>, skills: Vec<SkillView> }`
  `SkillView { id, name, description, on, acts_on_computer, broken: Option<String>, body_chars }`
  `SkillDetail { skill: SkillView, pack_id, pack_title, license, source_url, body, files: Vec<String> }`; `source_url` is `https://github.com/{repo}/tree/{commit}/{dir}` taken from the lock's original path where known, else the repository at the commit.

- [ ] **Step 1: Write the failing tests** (temp folders with small packs; the real vendored tree for one smoke test):

```rust
// loads_builtin_installed_and_yours_with_the_right_origins
// a_pack_folder_without_pack_json_is_listed_with_an_error
// default_on_follows_default_skills_json; a_stored_switch_wins; pack_off_hides_its_skills_but_keeps_their_switches
// broken_skills_are_listed_but_never_on
// skill_body_returns_the_exact_file_text; skill_body_of_an_unknown_id_is_a_ui_error
// skill_file_refuses_a_name_not_in_the_entry_and_any_path_with_dot_dot
// the_real_vendored_tree_loads_all_15_packs_and_at_most_40_skills_are_on
// loading_the_real_tree_takes_under_500_ms   (reads 15 pack.json files, not 900 SKILL.md files)
// commands: list_packs_shape_matches_ipc; set_skill_on_persists_across_a_reload
```
```ts
// ipc.test.ts: each new function calls the right command with the right argument keys
```

- [ ] **Step 2:** Run the tests. Expected: fail.
- [ ] **Step 3:** Implement. `AppState` gains `library: Library` and resolves the built-in folder from Tauri's resource directory (tests pass a path).
- [ ] **Step 4:** Run all tests and `tsc`. Expected: pass. Commit: `feat(skills): skill library, switches and commands`.

### Task 7: Running skills in a chat

**Files:**
- Create: `src-tauri/src/commands/chat_state.rs`
- Modify: `src-tauri/src/commands/{turn.rs,handlers.rs,tests.rs}`, `src-tauri/src/lib.rs`, `src/lib/ipc.ts` and its test

**Interfaces:**
- Consumes: `Library`, `Store` chat state, `ModelInfo.context_length` from the cached model list, `MAX_BODY_CHARS`.
- Produces:
  - `pub const SKILL_NOTE: &str` holding the spec's fixed note, exact.
  - `pub const MAX_ACTIVE_SKILLS: usize = 3`.
  - Wire building in `turn.rs` becomes: when any active skill fits, one system message containing `SKILL_NOTE`, then for each fitting active skill a system message `Skill: {id}\n\n{body}`; then, when a summary exists, a system message `Summary of the earlier conversation:\n\n{summary}`; then `sendable_messages`.
  - Fit rule: `(SKILL_NOTE.len() + sum of active bodies) / 4 > context_length / 2` means too long. Skills are dropped from the end of the activation order until the rest fit. A model missing from the cache is treated as fitting.
  - Commands:

| Command | Arguments | Returns |
|---|---|---|
| `chat_state` | `chat_id, model_id` | `ChatState { active_skills: Vec<ActiveSkill>, folder: Option<String>, compacted: bool }` with `ActiveSkill { id, name, too_long: bool, acts_on_computer: bool }` |
| `activate_skill` | `chat_id, skill_id, model_id` | `Result<ChatState, UiError>`; also appends the note `Using {id}` |
| `deactivate_skill` | `chat_id, skill_id` | `ChatState` |
| `chat_fullness` | `chat_id, model_id, draft_chars` | `Fullness { percent: u32, context_length: u64 }` |

  - `activate_skill` errors (kind `other`, retryable false), exact:
    - already three active: "A chat can have three skills active. Remove one first."
    - body over 60,000 characters: "This skill is too long to run in a chat."
    - does not fit the model: "This skill is too long for {model name}. Pick a model with a larger limit, or remove another active skill."
    - off, broken or unknown: "This skill is not available. Check it in Skills."
    - already active: returns the current state without a second note.
  - `chat_fullness.percent` = (fixed note and fitting skill bodies + summary + every sendable message's text + decoded text attachments + 1,000 per image + `draft_chars`) / 4, over `context_length`, as a rounded percentage, not capped at 100.

- [ ] **Step 1: Write the failing tests.**

```rust
// no_active_skill -> wire history is unchanged from Plan 1 (no system message)
// one_active_skill -> first wire message is SKILL_NOTE (exact text), second is "Skill: pack:name\n\n" + body, then the conversation
// two_skills_keep_activation_order
// notes_are_never_sent
// a_summary_is_sent_as_a_system_message_and_older_messages_are_not
// activate: fourth skill is refused with the exact copy; over-long body refused; off/broken/unknown refused
// activate: a skill that does not fit a small-context model is refused with the model's name in the message
// activate_twice_adds_one_note
// switching_to_a_small_model_marks_the_last_activated_skill_too_long_and_leaves_it_out_of_the_wire_history
// a_model_missing_from_the_cache_is_treated_as_fitting
// retry_turn_sends_the_same_skill_messages_as_send_message
// fullness: an empty chat with a 1,000-char draft on a 4,000-token model is 6 percent; active skills raise it; it can exceed 100
// the key-leak test covers the four new commands
```
```ts
// ipc.test.ts: the four new functions
```

- [ ] **Step 2:** Run `cargo test commands`. Expected: fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run all tests. Expected: pass. Commit: `feat(skills): run active skills ahead of the conversation`.

### Task 8: Edit and resend, compact, export

**Files:**
- Modify: `src-tauri/src/commands/{turn.rs,chats.rs,handlers.rs,tests.rs}`, `src-tauri/src/lib.rs`, `src-tauri/Cargo.toml` and `capabilities/default.json` (`tauri-plugin-dialog`, save and open-folder permissions only), `src/lib/ipc.ts` and its test

**Interfaces:**
- Produces:

| Command | Arguments | Returns |
|---|---|---|
| `take_last_user_message` | `chat_id` | `Result<TakenMessage { text, attachments: Vec<AttachmentInput> }, UiError>`; deletes that message and everything after it |
| `compact_chat` | `chat_id, model_id, keep: Option<String>` | `Result<TurnResult, UiError>` |
| `export_chat` | `chat_id, path` | `Result<(), UiError>`; writes Markdown to `path` |

  - `take_last_user_message` refuses while a turn is running in that chat ("Wait for the reply to finish, or stop it, before editing.") and when the chat has no user message ("There is nothing to edit in this chat.").
  - `compact_chat`: refused under four sendable messages ("This chat is too short to shorten."). Sends one non-streamed-to-UI request through the engine whose messages are a system message "Summarise the conversation below so it can be continued later. Keep decisions, file names, code that matters and open questions. Be brief." (plus "Make sure to keep: {keep}" when given) followed by the conversation without skill messages. If the conversation exceeds 60% of the model's context by the Task 7 estimate, the oldest messages are left out until it fits and the stored summary is prefixed with "The oldest messages were too long to include in the summary.\n\n". On `done`, stores the summary with `sent_from_seq` = one past the last summarised message, records usage, and appends the note "Summarised above this line". On any other outcome nothing changes.
  - `export_chat` Markdown: `# {title}`, then per row `## You` / `## {model id}` / `> {note text}` blocks in order, attachments listed by name, the summary (if any) under `## Summary of earlier messages`. Notes and pre-compaction messages are included.

- [ ] **Step 1: Write the failing tests.**

```rust
// take: returns the last user text and attachments and deletes it and later rows; earlier rows stay
// take: active skills are still active afterwards
// take: refused while a turn is running; refused with no user message (exact copy)
// compact: fewer than four messages refused (exact copy), no request made
// compact: success stores the summary, appends the note, records one request of usage; the next turn sends summary + later messages only
// compact: the summary request contains no "Skill:" system message even when a skill is active; the skill is still active after
// compact: keep text is included in the request
// compact: a failed request changes nothing in the store
// compact: an over-long conversation leaves out the oldest messages and prefixes the stored summary
// export: file contains the title, both roles, a note line, attachment names and the summary section
// export: an unwritable path returns a UiError and does not panic
```

- [ ] **Step 2:** Run `cargo test commands`. Expected: fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run all tests. Expected: pass. Commit: `feat(chat): edit last message, compact and export`.

### Task 9: Folder files for `@`

**Files:**
- Create: `src-tauri/src/folder.rs`, `src-tauri/src/commands/folder.rs`
- Modify: `src-tauri/Cargo.toml` (`ignore`), `src-tauri/src/commands/handlers.rs`, `src-tauri/src/lib.rs`, `src/lib/ipc.ts` and its test

**Interfaces:**
- Produces:
  - `pub fn list_files(root: &Path, query: &str, limit: usize) -> FileList` with `FileList { entries: Vec<FileEntry>, total: usize, cut: bool }`, `FileEntry { path: String /* forward slashes, relative */, is_dir: bool, bytes: u64 }`.
  - `pub fn read_file(root: &Path, rel: &str) -> Result<Vec<u8>, String>`; `pub fn folder_listing(root: &Path, rel: &str) -> Result<String, String>`.
  - Walk rules: respects `.gitignore`; skips hidden entries and `node_modules`, `target`, `dist`, `build`, `.git`; does not follow links; stops after 5,000 files with `cut: true`. `query` filters by case-insensitive substring of the path, ranking file-name matches before folder-path matches, then shorter paths first.
  - Boundary: `rel` is rejected when absolute or containing `..`; the resolved real path must start with the real path of `root`.
  - Commands: `set_chat_folder(chat_id, path: Option<String>)` (the path must exist and be a folder); `folder_files(chat_id, query) -> FileList` (limit 50 shown); `attach_folder_file(chat_id, path) -> AttachmentInput` (files are classified and limited exactly as Plan 1's attachments; a folder returns its listing as `{name}/ listing.txt`, up to 500 entries).

- [ ] **Step 1: Write the failing tests** (temp folders):

```rust
// lists_files_relative_with_forward_slashes
// respects_gitignore_and_skips_node_modules_target_dist_build_dot_git_and_hidden
// query_ranks_file_name_matches_first_then_shorter_paths
// stops_at_5000_files_with_cut_true
// a_symlink_to_outside_is_not_listed_and_cannot_be_read (cfg(unix))
// read_file_rejects_dot_dot_and_absolute_paths
// read_file_inside_the_root_returns_exact_bytes
// folder_listing_is_capped_at_500_entries_and_says_so
// an_unreadable_entry_is_skipped_without_error (cfg(unix))
// commands: set_chat_folder_refuses_a_missing_path_and_a_file; folder_files_with_no_folder_returns_an_empty_list
// commands: attach_folder_file_applies_the_200_000_byte_text_limit_with_Plan_1_copy
```

- [ ] **Step 2:** Run `cargo test folder`. Expected: fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run all tests. Expected: pass. Commit: `feat(folder): read-only file listing and attach for a chat's folder`.

### Task 10: Adding skills and installing extra packs

**Files:**
- Create: `src-tauri/src/skills/install.rs`
- Modify: `src-tauri/src/commands/{skills.rs,handlers.rs}`, `src-tauri/src/lib.rs`, `src/lib/ipc.ts` and its test

**Interfaces:**
- Consumes: `archive`, `copy_pack`, `load_lock` (the lock file is embedded with `include_str!`), `Library::reload`.
- Produces:

| Command | Arguments | Returns |
|---|---|---|
| `extra_packs` | | `Vec<ExtraPack { id, title, about, license, repo, installed: bool, needs_warning: bool }>` (the three non-bundled lock entries; `needs_warning` true only for `claude-red`) |
| `install_extra_pack` | `id` | `Result<PackView, UiError>` |
| `remove_pack` | `id` | `Result<(), UiError>` (installed packs only) |
| `preview_add` | `source: AddSource` | `Result<AddPreview { skills: Vec<SkillView>, skipped: Vec<String> }, UiError>` |
| `confirm_add` | `source: AddSource` | `Result<PackView, UiError>` (adds to `yours`) |
| `remove_skill` | `id` | `Result<(), UiError>` (skills in `yours` only) |

  `AddSource` is `{ "kind": "folder", "path" }`, `{ "kind": "zip", "path" }` or `{ "kind": "github", "url" }`. A GitHub URL may be `https://github.com/owner/repo`, `…/tree/<ref>` or `…/tree/<ref>/<sub/path>`; the ref defaults to the repository's default branch, resolved through `https://api.github.com/repos/{owner}/{repo}` and then pinned to the commit SHA returned for it.
  - Limits and exact errors (kind `other`): no `SKILL.md` found: "No skills were found there. A skill is a folder with a SKILL.md file."; more than 200 skills: "That source has more than 200 skills. Add a smaller folder."; total text over 2,000,000 bytes: "Those skills are too large to add."; not a GitHub URL: "That is not a GitHub link."; download failure: "Could not download from GitHub. Check your connection and the link."; checksum mismatch on an extra pack: "The downloaded pack did not match its recorded checksum, so it was not installed."
  - Skill discovery for adds: the source itself if it has `SKILL.md`, otherwise every folder with `SKILL.md` up to four levels down, excluding hidden folders.
  - A name already in `yours` is replaced by the new copy.
  - Installed extra packs: every skill starts off, for all three.
  - Everything is staged in a temp folder and moved into place only after validation.

- [ ] **Step 1: Write the failing tests.**

```rust
// add_folder_with_one_skill; add_folder_containing_three_skills; scripts are not copied
// add_zip; a zip with a ../ entry is refused and nothing is written
// add_github: wiremock for api.github.com (default branch + commit) and codeload; sub-path links add only that folder
// each limit returns its exact copy
// adding_a_skill_twice_replaces_it; remove_skill_deletes_only_that_skill and refuses built-in ids
// install_extra_pack: copies text files, writes pack.json, all skills off; remove_pack deletes it; removing a built-in pack is refused
// install_extra_pack_with_a_wrong_checksum_is_refused_and_leaves_nothing_behind
// a_failed_install_leaves_no_partial_folder
// extra_packs_lists_three_with_needs_warning_only_for_claude_red
// claude_red_style_skills_without_frontmatter_get_names_and_descriptions
```

- [ ] **Step 2:** Run `cargo test skills::install commands`. Expected: fail.
- [ ] **Step 3:** Implement. Compute and record the three extra packs' checksums in `skills.lock.json` with the vendoring tool (`--extras` writes checksums without vendoring them).
- [ ] **Step 4:** Run all tests. Expected: pass. Commit: `feat(skills): add your own skills and install extra packs`.

### Task 11: The `/` menu, commands and active skills

**Files:**
- Create: `src/features/menu/{PopMenu.tsx,match.ts,menu.css}`, `src/features/commands/{registry.ts,useSlashMenu.ts,Help.tsx,Usage.tsx}`, `src/features/skills/ActiveSkills.tsx`, tests beside each
- Modify: `src/features/chat/{Composer.tsx,Thread.tsx,useChat.ts}`, `src/features/shell/Shell.tsx`

**Interfaces:**
- Consumes: `listPacks`, `chatState`, `activateSkill`, `deactivateSkill`, `takeLastUserMessage` (Task 14 uses it), existing `Shell` handlers.
- Produces:
  - `matchRows(query: string, rows: Row[]): Row[]` with `Row { id, name, description, source, group: "Commands" | "Skills" }`. Order: name matches at the start of the name or of a word in it (split on `:`, `-`, `_`), then description-word matches; original order within each rank.
  - `<PopMenu rows groups active onPick onClose footer />`: `role="listbox"`, rows `role="option"` with `aria-selected`, Up/Down/Enter/Tab/Esc, click outside closes.
  - `commands: Command[]` with `Command { name, description, run(ctx) }` for `/new`, `/model`, `/skills`, `/usage`, `/compact`, `/export`, `/settings`, `/help`. Descriptions, exact: "Start a new chat"; "Switch the model for this chat"; "See and manage your skills"; "Show free requests left and what this chat has cost"; "Shorten this chat to save requests and cost"; "Save this chat as a Markdown file"; "Open Settings"; "List commands and keyboard shortcuts".
  - Menu footer: "↑↓ move", "Enter run", "Tab complete", "Esc close", "Running a skill sends a message", and a "Manage skills" button.
  - Thread renders `kind: "note"` rows as a quiet centred line.
  - `<ActiveSkills>` chips above the message box: skill name, a "Remove {name}" button, and for `too_long` the text "too long for this model".

Behaviour: the menu opens when the box's first character is `/` and the caret is in the first word. Picking a command runs it and clears the box. Picking a skill calls `activateSkill`, then sends the rest of the box as the message, or "Start." when empty, through the existing send path; an `activateSkill` rejection shows its message with `role="alert"` and sends nothing. With no match, Enter sends the text as a normal message. `/compact` shows a confirmation "Shorten this chat? This uses one request." with "Shorten" and "Cancel"; `/export` opens the save dialog with the default name `{title}.md`.

- [ ] **Step 1: Write the failing tests.**

```ts
// match: "/br" finds superpowers:brainstorming by word start; "/plan" ranks writing-plans above a description-only match; ":" "-" "_" split words
// PopMenu: arrows move, Enter picks, Tab completes without running, Esc closes, aria-selected follows
// slash menu opens only when "/" is the first character; "hello /" does not open it
// only skills that are on, in a pack that is on, and not broken are listed; groups are Commands then Skills
// picking /new starts a new chat and clears the box
// picking a skill with text calls activateSkill then send with the text; with no text sends "Start."
// a rejected activateSkill shows its message as an alert and sends nothing
// no match + Enter sends the literal text
// note rows render as a quiet line and are not message bubbles
// ActiveSkills: chip per skill, remove calls deactivateSkill, too_long shows the exact text
// /compact asks first with the exact copy; Cancel makes no request
// /help lists every command name and description
```

- [ ] **Step 2:** Run `pnpm test`. Expected: fail.
- [ ] **Step 3:** Implement to the mockup's "Typing /" screen.
- [ ] **Step 4:** Run `pnpm test`, `tsc`, `pnpm build`. Expected: pass. Commit: `feat(ui): slash menu, commands and active skills`.

### Task 12: The `@` menu and opening a folder

**Files:**
- Create: `src/features/mentions/{useMentionMenu.ts,useMentionMenu.test.tsx}`
- Modify: `src/features/chat/Composer.tsx`, `src/features/shell/{Shell.tsx,Sidebar.tsx}`, `package.json` (`@tauri-apps/plugin-dialog`)

**Interfaces:**
- Consumes: `PopMenu`, `setChatFolder`, `folderFiles`, `attachFolderFile`, `chatState`, the Composer's existing attachment chips and refusals.

Behaviour: `@` opens the menu when typed at the start of the box or after whitespace; the query is the text from `@` to the caret; it closes on whitespace, Esc, or deleting the `@`. Rows show the file name, its folder path in secondary text, and its size, or "folder, lists its files". The header reads `Files in {folder name} matching "{query}"` and "{shown} of {total}", with "list cut at 5,000 files" when cut. Enter attaches through `attachFolderFile` and removes the `@query` text from the box; refusals appear where attachment refusals already do. With no folder: one row "Open a folder to mention its files" which opens the system folder dialog, calls `setChatFolder`, and reopens the menu. The sidebar gains a "Folder" group showing the folder's name with "Change" and "Close folder" actions, or an "Open a folder" button. Requests are debounced by 120 ms and stale responses are ignored. A new chat starts with the last folder used (`localStorage` key `lastFolder`), set on the chat when it is created.

- [ ] **Step 1: Write the failing tests.**

```ts
// "@" at the start and after a space opens the menu; "a@b" does not
// typing after "@" queries folderFiles with that text, debounced; a stale response does not replace a newer one
// Enter attaches via attachFolderFile, adds a chip, and removes "@rou" from the box leaving the rest of the text
// a refused attachment shows the core's message as an alert and adds no chip
// with no folder the single row opens the folder dialog and sets the folder
// the header shows the folder name, the query and the counts; cut shows the exact note
// the sidebar shows the folder name and "Close folder" clears it
// a new chat inherits lastFolder
```

- [ ] **Step 2:** Run `pnpm test`. Expected: fail.
- [ ] **Step 3:** Implement to the mockup's "Typing @" screen.
- [ ] **Step 4:** Run `pnpm test`, `tsc`, `pnpm build`. Expected: pass. Commit: `feat(ui): at-mention menu for files in an opened folder`.

### Task 13: The Skills screen

**Files:**
- Create: `src/features/skills/{SkillsScreen.tsx,PackCard.tsx,SkillDetail.tsx,AddSkill.tsx,MorePacks.tsx,RedWarning.tsx,skills.css}` and tests
- Modify: `src/features/shell/{Shell.tsx,Sidebar.tsx}`, `src/features/settings/Settings.tsx`

**Interfaces:**
- Consumes: `listPacks`, `getSkill`, `setSkillOn`, `setPackOn`, `skillFile`, `extraPacks`, `installExtraPack`, `removePack`, `previewAdd`, `confirmAdd`, `removeSkill`, `activateSkill`, the dialog plugin (folder and ZIP pickers), `openAuthUrl`-style external open restricted to `https://github.com/*` (add that capability scope).

Exact copy: screen title "Skills"; lede "Skills are instructions {APP_NAME} follows for a kind of task. {n} are on. Fewer is better on small free models."; tabs "Installed" and "More packs"; button "Add a skill"; pack meta "{n} skills, {m} on"; origins "built in", "installed", "added by you"; the acts note from the spec, exact; detail buttons "Use in this chat" and "Attach to this chat"; add dialog options "A folder on this computer", "A ZIP file", "A GitHub link"; add confirmation "Add {n} skills" (or "Add 1 skill"); "Remove"; sidebar row "Skills" with "{n} on"; Settings entry "Open-source licenses". The claude-red dialog copy is in the spec, exact.

Behaviour: Installed lists packs as cards (collapsed shows the first three skills and "Show all {n}"); a search field filters skills across packs by name and description and expands matching packs; switches update immediately and revert with an alert if the command fails. Skill detail renders the body with the existing Markdown component, shows the pack, license, a "View source" link, the acts note when it applies, and its other files. "Use in this chat" activates the skill in the current chat and returns to it (creating a chat first if none is open). More packs lists the three extra packs with Install or Remove and shows the warning dialog before installing `claude-red`. The licenses view lists every pack with its license id and the text of its license files (read through a new command `pack_licenses(id) -> Vec<{ name, text }>`, added to `commands/skills.rs` with a test).

- [ ] **Step 1: Write the failing tests.**

```ts
// lede shows the count of skills that are on, using APP_NAME
// a pack card shows title, counts, license and origin; "Show all" expands
// toggling a skill calls setSkillOn and updates the count; a failed call reverts and alerts
// toggling a pack calls setPackOn and greys its skills without changing their own switches
// search filters across packs and expands matches; no match shows "No skills match."
// a broken skill shows its reason and has no switch
// detail shows the acts note only when acts_on_computer; "Use in this chat" calls activateSkill
// add: each of the three sources calls previewAdd, shows the found skills and skipped files, then confirmAdd
// add: a rejected preview shows the core's exact message
// More packs: installing claude-red opens the warning first; "Cancel" installs nothing; "Install, switched off" calls installExtraPack
// installed extra packs show Remove; built-in packs never do
// licenses view lists every pack and shows license text
```
```rust
// pack_licenses returns the pack's license files; unknown pack is a UiError
```

- [ ] **Step 2:** Run the tests. Expected: fail.
- [ ] **Step 3:** Implement to the mockup's "The Skills screen".
- [ ] **Step 4:** Run all tests, `tsc`, `pnpm build`. Expected: pass. Commit: `feat(ui): skills screen with add, extra packs and licenses`.

### Task 14: Chat habits, README and version

**Files:**
- Create: `src/features/chat/{Fullness.tsx,Queued.tsx}` and tests
- Modify: `src/features/chat/{Composer.tsx,Thread.tsx,useChat.ts}`, `src/features/shell/Shell.tsx`, `README.md`, `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` (version `0.2.0`)

**Interfaces:**
- Consumes: `takeLastUserMessage`, `chatFullness`, `stop`, `useChat`.

Behaviour and exact copy:
- A pencil button "Edit message" on the latest user message only, hidden while a reply is running. It calls `takeLastUserMessage`, reloads the thread, and puts the text and attachment chips back in the message box. If the text began with a `/skill-id ` that is active, only the rest returns.
- The message box stays enabled during a reply. Sending then queues one message shown above the box as "Queued: {first 80 characters}" with a "Remove queued message" button; it is sent when the reply settles as `done`, and kept (not sent) when the reply fails or is interrupted. A second send replaces it. Attachments queue with it.
- Esc stops the reply when no menu is open and a reply is running; otherwise it does nothing new.
- The header shows "Chat is {n}% full", recomputed after each turn, on model change, on skill change and (debounced 300 ms) while typing. From 70 it uses the amber text colour and adds "Use /compact to shorten it".
- README: Features table gains rows for the `/` menu, `@` files, the Skills screen and the bundled packs (name each pack with its license and link); the Privacy section says GitHub is contacted only when the user installs or adds skills from it; the Roadmap marks this stage released as 0.2 and renumbers the rest; a "Bundled skill packs" section credits every pack's repository and license.

- [ ] **Step 1: Write the failing tests.**

```ts
// the pencil shows only on the latest user message and not while streaming
// editing calls takeLastUserMessage, reloads, and restores text and chips to the box
// a leading "/pack:skill " for an active skill is stripped from the restored text
// sending during a reply queues; the queued line shows the first 80 characters; a second send replaces it
// the queued message is sent after "done" and kept after "failed" and "interrupted"
// Remove queued message clears it
// Esc during a reply calls stop; Esc with the slash menu open only closes the menu
// Fullness shows the percent; 70 is amber with the /compact hint; 69 is not
// fullness is requested again after a turn, a model change and a skill change
```

- [ ] **Step 2:** Run `pnpm test`. Expected: fail.
- [ ] **Step 3:** Implement, then update the README and bump the three version fields.
- [ ] **Step 4:** Run every check: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `pnpm exec tsc --noEmit`, `pnpm test`, `pnpm build`, `cargo build`. Expected: pass. Commit: `feat(chat): edit and resend, queued message, Esc to stop and fullness meter; 0.2.0`.
