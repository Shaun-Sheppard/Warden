# Warden and prr

This repository holds two front ends over one Rust core:

- **Warden** (`app/`): a desktop app (Tauri + React) that watches Azure DevOps for new pull requests, has Claude Code review each one, and posts the result unattended.
- **prr** (repository root): the original command-line tool, documented further down.

## Warden

### Install

Download the latest installer from the [Releases page](https://github.com/Shaun-Sheppard/Warden/releases).

- **Mac:** open the `.dmg` and drag Warden to Applications. It runs on Apple Silicon and Intel Macs.
- **Windows:** run the `-setup.exe`.

The installers are not code-signed, so the system warns on first launch:

- **Mac:** if you see "Warden can't be opened" or "is damaged", open System Settings → Privacy & Security and choose **Open Anyway**, or run `xattr -dr com.apple.quarantine /Applications/Warden.app` in Terminal.
- **Windows:** on the "Windows protected your PC" screen choose **More info → Run anyway**.

You also need `git` and the [Claude Code CLI](https://claude.com/claude-code) installed and signed in (`claude auth login`), and an Azure DevOps personal access token with Code (Read & Write) and Work Items (Read).

The Windows build has not been tested on a real machine yet; please report problems.

### Updates

Installed copies check GitHub for a newer release when they start and every six hours, and show an "Update and restart" banner when one exists. Nothing is installed without the user choosing to. Settings → About has a manual "Check for updates".

Update packages are signed with a Tauri updater key. The public half is in `app/src-tauri/tauri.conf.json`; the private half is the `TAURI_SIGNING_PRIVATE_KEY` secret on the GitHub repository and must be kept safe: without it, existing installs cannot be updated.

### Signing on macOS

Releases are signed with a self-signed certificate ("Warden Self-Signed"). It is not an Apple Developer ID, so the first-launch warning remains, but it gives every version the same identity: after choosing **Always Allow** once on the Keychain prompt, updates no longer ask again.

The certificate's private half is the `MACOS_CERTIFICATE` and `MACOS_CERTIFICATE_PASSWORD` secrets on the GitHub repository; the public certificate is in `app/src-tauri/signing/`. If it is ever replaced, every user gets one more Keychain prompt. For local builds, `app/scripts/sign-local.sh` signs the built app with the same certificate when it is set up in `~/.tauri`.

### Releasing a new version

Set the new version in `app/src-tauri/tauri.conf.json` (and, to keep them in step, `app/package.json` and `app/src-tauri/Cargo.toml`), commit, then push a matching tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The Release workflow builds the Mac and Windows installers and publishes them.

### Build and run

```bash
cd app
npm install
npx tauri build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
```

That produces `target/release/bundle/macos/Warden.app`. The `--config` part skips signing an update package, which needs the release key.

For development, `npx tauri dev` runs the app with live reload. `npm run dev` on its own serves the UI in a browser against sample data (`?setup`, `?idle` and `?error` show other states).

Requires Rust, Node, `git`, and the Claude Code CLI signed in (`claude auth login`).

### What it does

1. **Setup:** organisation and personal access token (stored only in the system keychain), which projects and whose PRs to watch, and a check that Claude Code is installed and signed in. If `prr` was configured before, its organisation, project and authors are pre-filled and its stored token is reused.
2. **Monitoring:** every 30 seconds to 15 minutes it lists active PRs in the chosen projects (all projects if none are named), skipping drafts and people not on the list. PRs already open when monitoring first starts are left alone unless you choose otherwise in setup.
3. **Review:** each new PR is cloned into `~/.cache/prr/repos` and reviewed by `claude -p` with read-only tools. Claude is asked to (a) check the change against the acceptance criteria of the work items linked to the PR, marking each met, not met or unclear, (b) find defects the change introduces, and (c) look specifically for security problems. The result is posted as one PR comment: decision, summary, acceptance criteria, and issues grouped Critical / Major / Minor. An unmet criterion blocks approval. Reading work items needs the token's Work Items (Read) scope; without it the review still runs and says the criteria were not checked. The instructions can be edited under Settings → Review prompt.
4. **Voting (off by default):** with "Vote, and auto-complete clean PRs" on, a PR with no critical or major issues is approved and set to auto-complete; otherwise the vote is "Waiting for author". The comment for an approving review is posted as an already closed thread, so it does not hold up the merge where branch policy requires comments to be resolved; a rejecting review's comment stays active.
5. **New commits:** a reviewed PR, approved or rejected, is reviewed again whenever new commits are pushed, so a decision never covers code that was not looked at. With **Don't review pull requests again once approved** (Settings → PR review), an approved PR (by Warden, Approve anyway, or with your approval vote) is left alone instead, for example when a pipeline failure after approval is fixed. New comments on the PR do not trigger a review.
6. **Several people running Warden:** before reviewing, a copy leaves a hidden marker on the pull request (the `Warden.Review` property, not visible in Azure DevOps) saying who is reviewing which commit. Other copies skip that PR, show it under "Being reviewed elsewhere", and do not review a commit another copy has already reviewed. A marker left by a copy that quit mid-review expires after 20 minutes. "Review again" overrides the marker. Azure DevOps has no lock, so two copies that start within the same couple of seconds can, rarely, both review.
7. **History:** one row per pull request, showing its latest review; earlier reviews of the same PR are listed on the review page. Each review is kept for 90 days with its issues, the exact comment posted, a timeline and the log. The sidebar counts and the default list cover the last 7 days. Failed reviews are shown with the reason and retried up to three times; "Review again" re-runs one on demand.

### How Claude is confined

A pull request is untrusted input: its code, description, linked work items and every file in its repository could be written to manipulate the reviewer. Warden therefore runs Claude with:

- **No shell.** Reviews have only `Read`, `Grep` and `Glob`. Git is not available to it at all, because even "read-only" git commands can write files (`--output`) or read files outside the repository (`--no-index`). Warden prepares the diff, changed-file list and commits itself and passes them as files.
- **File access limited to the repository copy** (plus that folder of prepared files). It cannot read your home folder, keys or other projects.
- **No settings from the repository.** A repository's own `.claude/settings.json` can define hooks that run commands; these, and the repository's `CLAUDE.md`, skills and plugins, are not loaded. No MCP servers are loaded either.
- **Fixes** add `Edit` and `Write`, limited to the fix's own working folder, and cannot touch git or tool configuration files.

This relies on the Claude Code CLI's `--restricted`, `--safe-mode` and `--strict-mcp-config` options. An older CLI without them is refused with a message to update, so a review never runs unconfined.

Confinement limits what a manipulated review can *do*; it cannot guarantee the *verdict* is right. Two things guard the verdict: Claude is told to report any text addressed to the reviewer, and Warden scans the change, description and work items for such text itself. Either one blocks automatic approval, marks the review, and says so in the posted comment. Neither is proof against a carefully worded pull request, so automatic approval of other people's pull requests remains a matter of trust.

Other protections: the Azure DevOps token is offered by git only to `dev.azure.com`; the app window runs under a content security policy; and Warden shows up front when the installed Claude Code is too old to be confined.

`tests/live_claude.rs` runs a real review against a hostile repository (a command hook, plus instructions to read and write outside the repository and approve) and checks that none of it works: `cargo test --test live_claude -- --ignored`.

### Board triage

Settings has four sections. **General** holds the connection, check interval, notifications, Claude Code and appearance. **PR review** and **Triage** hold what applies to every project (dry runs, prompts, fixes at a time). **Projects** holds everything else, one card per project: whether its pull requests are reviewed, whose, and whether Warden votes and completes them; whether its board is triaged, which states and work item types count, whether items are fixed automatically, and its area paths and repositories. Settings from earlier versions are moved into project cards automatically.

Turn on **Triage my work items** for a project to have Warden look at the work items assigned to you there, in the "to do" states and types you choose (by default Bugs in `To Do` or `New`), in the project's current sprint (switch off **Current sprint only** to include later sprints and the backlog).

An item with child tasks assigned to you only counts while at least one of those tasks is in a to-do state: while your task is blocked, in progress or in review, the item is left alone. An item you move to the board's in-progress state yourself (Committed, the taskboard's In Progress column) shows under **Fixing** marked "By hand"; one moved anywhere else (In Review, Blocked, Done) or reassigned drops off at the next check, paused or not. Each card on the board has a **⋯** menu: **Open**, **Remove**, and, for items in AI can fix or For a person, **New session**. New session fetches the latest base branch into your own copy of the repository, creates a worktree in `.claude/worktrees/<id>-<title>` on a new branch from it (copying your local-only files, as for Warden's fixes), and opens the Claude desktop app there with `/fix <id>` ready to send. **Remove from Warden** (in an item's details, or the ⋯ menu) forgets an item; the next check adds it back only if it still qualifies.

For an item that sits under another (a "Dev" task under a PBI, say), Warden shows the parent with it and gives Claude the parent's description and acceptance criteria too, since that is usually where the detail is.

For each item, Claude reads it, and, if its area path is mapped to a repository, looks through that repository's code (read-only, confined as below). It rates the **risk** (low, medium, high), whether an **AI agent could fix it** on its own, and its **confidence**. An item goes to an agent only when it is low risk, suitable, rated with high confidence, mapped to a repository, and free of text aimed at automated tools. Everything else is for a person.

The results are kept in Warden. With **Tag and comment on items** on, Warden also tags each item `warden-ai-fixable` or `warden-needs-human`, and comments on the ones for a person, mentioning you, with the reason. The results are listed on the **Triage** page. Items are triaged a few at a time, after pull requests, and each only once unless you choose "Triage again". Triage itself never changes code.

**Area paths and repositories** (in each project's card) say where each area's work belongs, one card per area with how a fix gets ready to build: an area covers everything below it, and the most specific mapping wins. The token needs **Work Items (Read & Write)**.

**Triage dry run** (on by default, separate from the pull request Dry run) sends nothing to Azure DevOps: no tags or comments, and fixes are tested but never pushed. With it off and **Tag and comment on items** off, triage results stay in Warden while fixes still push and open pull requests. **Triage prompt** lets you rewrite how Claude rates risk and suitability, like the review prompt; Warden still adds the item, the repository and the answer format. **Work item types** and **states** offer the names your projects actually use, including custom types such as `Story Bug`.

### Fixing work items

The Triage page shows your items as a board (To triage, AI can fix, For a person, Fixing, In review, Failed review, Fixed) or a list; click one for its details and actions. **Fix with Claude** fixes a triaged item (any item whose area is mapped to a repository). Items triage left for a person show **Fix anyway** instead, and can be moved to **AI can fix** (or back) so automatic fixes pick them up. With **Fix automatically** on for a project, Warden does this by itself for the items triage marked AI-fixable. Each kind of work can use its own Claude model and effort level (Settings → PR review and Triage): reviews, triage, simple fixes and complex fixes. Triage rates each fix simple or complex (you can change it in an item's details); items that came back after a fix, and items fixed anyway, count as complex. A simple fix that fails is tried once more on the complex model. **Local pre-triage** (Settings → Triage, optional) has a model running on your computer through Ollama read each item's text first; items it is sure need a person skip Claude's triage and are marked "Screened locally", and everything else goes to Claude. With **Use the local model for all triage**, the local model rates every item from its text alone and Claude is not used for triage (fixes still use Claude). Automatic fixes wait until every item has been triaged, then start together. The pause button in the sidebar's monitoring box pauses everything: what is being reviewed, triaged or fixed finishes, nothing new starts, and the board is still kept up to date until you resume. **Fixes at a time** (1 to 5) sets how many run side by side; each has its own worktree, and git work on a shared clone takes turns.

For each item Warden:

1. Creates a new branch, `warden/<id>-<title>`, from the mapped branch, in its own worktree under `~/.cache/prr/item-fixes`.
2. Gets that copy ready, as set in the mapping's **Fix setup**: copies local-only files from your own checkout (settings such as `appsettings.Development.json` or `environment.ts`, either listed one by one or, with **Copy all local-only files**, everything git does not track apart from dependencies and build output), then runs the optional **setup command** (e.g. `dotnet restore && cd Acme.Portal && npm ci`). Copies and whatever setup writes are kept out of every commit, even files git tracks; your own checkout is only read.
3. Has Claude fix the item, add or update tests, then find and run the repository's build and tests itself until they pass. Claude can read and edit files there and run build and test commands only (`dotnet restore/build/test`, `npm ci`, `npm test`, `npm run test/build/lint`, `npx ng test/build/lint`, `npx jest/vitest/tsc`, `yarn test`, `pnpm test`, optionally after a `cd`); anything else, and committing or pushing, is refused. Build output its test runs leave behind (`bin`, `obj`, `TestResults`, `dist`…) is kept out of the commit.
4. Only if Claude reports that the build and tests pass, commits as you, pushes the new branch (never forcing) and opens a pull request into the mapped branch linked to the work item. If they fail, or Claude finds no tests, nothing is pushed and the item shows why.

The item moves on the board as the fix goes: to its in-progress state (e.g. **Committed**, the taskboard's In Progress column) when Claude starts, back to where it was if the fix fails before a pull request is open, and to **In Review** once the pull request is approved, with a comment on the item that mentions whoever raised it, says it is ready for testing and lists what was done. If an item comes back to your board after that (it failed testing and was assigned back to you in a to-do state, say), it is triaged again, and its earlier fix is kept as history. Triage and fixes read the item's latest comments, so QA's notes are taken into account, and a second fix gets a branch of its own. Warden follows each fix's pull request on every check; one that is rejected or waiting for the author shows under **Failed review**, ready for **Address PR comments**. **Approve anyway** (on a failed review, in History or the item's details) asks for your reason, posts it as a reply on Warden's review comment and resolves it, then approves the pull request and sets auto-complete.

The pull request is then reviewed by Warden like any other (if your author filter includes you), by a separate run of Claude. With **Triage dry run** on, steps 1–3 run but nothing is committed, pushed or opened, so you can see the change and the test results first.

**Pull request comments.** Each fix records its pull request number. To act on a rejected review or reviewers' comments, search the Triage page for the PR number (e.g. `812`) and choose **Address PR comments**. Claude reads the open comment threads, changes the code on the same branch and runs the tests as before, and Warden pushes a new commit and posts a short summary on the pull request. The new commit is then reviewed again.

Building and testing runs the repository's code, and Claude's change to it, on your computer with your user's permissions. Only map repositories you trust.

### Fix with Claude

On a review with issues, tick the ones to fix and choose "Fix selected". Claude edits a private copy of the pull request's branch (a separate git worktree under `~/.cache/prr/fixes`), with file-editing tools but no ability to commit, push or run commands. The result appears under **Fixes** in the sidebar with Claude's own report and the full diff.

Nothing reaches the repository until you choose **Approve and push**. Warden then commits to the PR's branch as you, without forcing, and the new commit is reviewed like any other push. If the branch has moved on since the fix was prepared, the push is refused. **Discard** throws the changes away.

Warden cannot build the project or run its tests, so a fix is unverified until your pipeline runs. Pushing is blocked while Dry run is on.

A system notification is shown when a new pull request starts being reviewed, when the review finishes (with the decision and what was done), and when an update is available; turn them off under Settings → General.

**Keep the computer awake** (Settings → General) stops idle sleep while Warden runs, so checks carry on while you are away; the screen can still turn off and lock. Closing the window keeps monitoring running; the tray icon has Open, Check now, Pause and Quit, and the monitoring box at the top of the sidebar shows when the next check is due, with a pause/play button. **Dry run** (Settings) reviews without posting, voting or completing.

Settings, history and tracking live in the app data directory (`~/Library/Application Support/dev.warden.app` on macOS).

### Layout

| Path | What |
|---|---|
| `src/` | Shared core: Azure DevOps client, git, Claude runner, review parsing, keychain |
| `app/src-tauri/src/engine.rs` | Monitoring loop and decision rules |
| `app/src-tauri/src/main.rs` | Commands exposed to the UI, tray, window handling |
| `app/src/` | React UI (`mock.ts` is the sample data used in a browser) |

# prr

AI-assisted pull request review for Azure DevOps, from the command line.

`prr` lists your active PRs, runs a review of one using the Claude Code CLI in headless mode, shows the findings, and posts the comments you choose back to the PR. Nothing is written to Azure DevOps without a per-comment choice **and** a final `y/N` confirmation.

## Requirements

- macOS (Apple Silicon is the primary target)
- Rust toolchain (to build)
- `git` and `claude` (Claude Code CLI, signed in) on your `PATH`
- An Azure DevOps Personal Access Token with **Code (Read & Write)** scope

## Install

```bash
cargo install --path .
```

## Setup

1. Create the config with `prr init`. It asks for your organization (a pasted `https://dev.azure.com/...` URL works) and project; run it inside an Azure DevOps clone and it pre-fills them from `origin`. It writes `~/.config/prr/config.toml`, which you can also edit by hand:

   ```toml
   organization = "my-org"
   project = "my-project"
   default_repo = "my-repo"          # optional: `prr list` shows only this repo unless --repo/--all
   authors = ["Shaun Sheppard"]      # optional: only show PRs raised by these people (name or sign-in email)

   [repos]                            # optional: review in an existing local clone instead of prr's own
   my-repo = "~/code/my-repo"

   [review]
   prompt_file = "~/.config/prr/review-prompt.md"   # optional: replaces the default review guidance
   timeout_seconds = 600                            # optional, default 600
   comment_prefix = "🤖 AI-assisted review:"        # optional: marker prepended to posted comments
   mention_author = true                            # optional, default true: tag the PR's author in posted comments
   ```

2. Store your PAT in the macOS Keychain:

   ```bash
   prr auth login
   ```

   The token is read with hidden input, validated against Azure DevOps, and stored only in the Keychain (service `prr-azure-devops`). It is never written to disk or printed.

## Usage

Run `prr` with no arguments to open the full-screen browser:

- **Left column:** active PRs, showing ID, title (truncated to fit) and author. `●` marks a review in progress, `✓` a PR with a saved review, `✗` a failed review.
- **Right column:** the selected PR's details, then live review progress (each step, plus what Claude is reading and running), then the verdict, summary and comments.

| Key | Action |
|---|---|
| `↑` / `↓` (or `k` / `j`) | Select a PR |
| `Enter` | Review the selected PR, after a `y`/`n` confirmation |
| `p` | Choose comments to post for the selected PR's review (same per-comment prompts and final `y/N` as `prr review`) |
| `f` | Cycle the list: all / mine / reviewing |
| `r` | Refresh the list |
| `PgUp` / `PgDn` | Scroll the right column |
| `q` | Quit (press twice while a review is running) |

The individual commands are still available:

| Command | What it does |
|---|---|
| `prr` | Open the full-screen PR browser. |
| `prr init [--force]` | Create the config file interactively. Refuses to overwrite an existing one without `--force`. |
| `prr auth login` / `prr auth logout` | Store / remove the PAT in the Keychain. |
| `prr list [--repo X] [--all] [--mine] [--reviewing] [--author "A,B"]` | Table of active PRs. `--mine --reviewing` shows PRs matching either. `--author` overrides `authors` from the config. |
| `prr auto [--interval N] [--once] [--dry-run] [--include-existing] [--yes]` | Unattended mode; see below. |
| `prr review <id>` | Review the PR, then choose what to post. |
| `prr review <id> --fresh` | Delete prr's clone of the repo and clone it again before reviewing. |
| `prr review <id> --dry-run` | Review and display only. Never prompts to post, makes no write calls. |
| `prr show <id>` | Show the last saved review for the PR without re-running it. |

### Auto mode

`prr auto` checks for new pull requests every `--interval` minutes (default 10), reviews each one once, and posts the review as a single PR comment (decision, summary, and the issues grouped by severity with their `file:line`) **without asking**. It is the only way `prr` posts without the per-comment and final confirmations, so:

- It asks once at start-up before it begins posting (`--yes` skips this, for unattended launches).
- The first time it runs, PRs that are already open are left alone; only PRs opened afterwards are reviewed. Pass `--include-existing` to review the open ones too.
- It respects `default_repo` and `authors`, and skips draft PRs.
- Each PR is reviewed once. Later pushes to the same PR are not re-reviewed, and nothing is ever posted twice (handled PRs are recorded in `~/.cache/prr/auto-state.json`).
- A PR whose review fails is retried on later checks, up to 3 attempts.
- `--dry-run` reviews but never posts and records nothing. `--once` does a single check and exits, for cron or launchd.

#### Approving and auto-completing

Off by default. With `prr auto --approve --autocomplete` (or `approve = true` / `autocomplete = true` under `[auto]` in the config), a PR whose review finds **no critical or major issues** and is not rejected is, after its comments are posted:

- approved with your vote: "Approved" if there were no issues at all, "Approved with suggestions" if only minor ones;
- set to auto-complete, so Azure DevOps merges it once its branch policies pass.

PRs with any critical or major issue get comments only. Optional `[auto]` settings: `merge_strategy = "squash"` (or `noFastForward`, `rebase`, `rebaseMerge`; the repository default if unset) and `delete_source_branch = true`.

The approval is recorded as yours, on the strength of an AI review alone. Whether it counts towards a merge depends on the repository's branch policies (for example, policies often do not count an author's approval of their own PR).

### What `prr review` does

1. Fetches the PR details from Azure DevOps.
2. Gets the code. `prr` keeps its own clone of each repo under `~/.cache/prr/repos/`: it clones on the first review of a repo, fetches the PR's source and target branches on later ones, and checks out the PR's source branch there. It authenticates with your PAT, passed to git through the environment so it is never written to disk or shown on a command line. Your own working copies are not involved.
3. Runs `claude -p` on a copy of the PR's files with only `Read`, `Grep` and `Glob`, confined as described under "How Claude is confined". The diff, changed-file list and commits are handed to it as files.
4. Parses the JSON result (one retry if it is invalid), saves it to `~/.cache/prr/reviews/<id>-<timestamp>.json`, and displays the verdict, summary and comments.
5. For each comment, and then for an overall summary comment: `[p]ost / [e]dit / [s]kip / [q]uit`. Edit opens `$EDITOR`. Quit abandons the whole run and posts nothing.
6. Lists what was chosen and asks `Post N comments to PR #<id>? [y/N]` (default No).
7. Posts each approved comment and reports success or failure per comment, with a link to the PR.

Inline comments are attached to the file and line on the new (source branch) side of the diff. Comments without a file are posted as general PR comments.

### Reviewing in your own clone

If a repo is mapped under `[repos]`, `prr` reviews in that clone instead, using your normal git credentials. It only runs `git fetch` to update `origin/*` remote-tracking refs there; your working tree and local branches are never touched.

### Project conventions

If the repo has a `REVIEW.md` in its root **on the PR's target branch**, its contents are included in the review prompt as project conventions.

If Claude's output cannot be parsed even after the retry, the raw output is saved under `~/.cache/prr/failed/`.

## Development

```bash
cargo test
```

Unit tests cover config parsing, review JSON parsing/validation, prompt building and API request building. `tests/managed_clone.rs` exercises the clone/update/checkout cycle against a local git remote. `tests/posting.rs` runs the posting flow against a mocked Azure DevOps API and verifies that `--dry-run`, quitting, skipping and declining the confirmation post nothing.
