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
4. **Voting (off by default):** with "Vote, and auto-complete clean PRs" on, a PR with no critical or major issues is approved and set to auto-complete; otherwise the vote is "Waiting for author".
5. **New commits:** a reviewed PR, approved or rejected, is reviewed again whenever new commits are pushed, so a decision never covers code that was not looked at. New comments on the PR do not trigger a review.
6. **Several people running Warden:** before reviewing, a copy leaves a hidden marker on the pull request (the `Warden.Review` property, not visible in Azure DevOps) saying who is reviewing which commit. Other copies skip that PR, show it under "Being reviewed elsewhere", and do not review a commit another copy has already reviewed. A marker left by a copy that quit mid-review expires after 20 minutes. "Review again" overrides the marker. Azure DevOps has no lock, so two copies that start within the same couple of seconds can, rarely, both review.
7. **History:** every review is kept with its issues, the exact comment posted, a timeline and the log. Failed reviews are shown with the reason and retried up to three times; "Review again" re-runs one on demand.

Closing the window keeps monitoring running; the tray icon has Open, Check now, Pause and Quit. **Dry run** (Settings) reviews without posting, voting or completing.

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
3. Runs `claude -p` in the clone with a read-only tool allowlist (`Read`, `Grep`, `Glob`, `git diff`, `git log`, `git show`), asking it to review `git diff origin/<target>...origin/<source>`.
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
