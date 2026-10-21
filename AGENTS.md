<!-- Generated from tmtroot/agents.tmt. Edit that, then `tomet export .`. -->

# Catasto

## Core Philosophy & Principles

### Precedence of Instructions

The rules in this document serve as the project baseline and defaults. Explicit instructions given by the user in a conversation always take precedence over these defaults.

### User-First Design over Developer Convenience

Always prioritize the end-user experience, syntax ergonomics, and API design.
If there is tension between developer or implementation convenience and user simplicity, user simplicity must always win. Never compromise user experience just because an implementation is easier.

### Resist Path-of-Least-Resistance & Legacy Bias

The existing implementation is never a justification or reason for new design choices.

- Do not justify decisions with "it is already implemented this way, so let's stick with it."
- Prioritize architectural beauty, elegance, and long-term correctness over the path of least resistance.
- Understand the existing context thoroughly, but never take shortcuts or settle for lazy patches.

### Transparent Execution

Always state what you intend to do before doing it. Never execute non-trivial actions or commands silently.

## Single Source of Truth & Documentation

### Never edit `.md` directly

All Markdown files (`README.md`, `AGENTS.md`, `CONTRIBUTING.md`) are generated build artifacts, not source files. `CLAUDE.md` is the one exception: it holds only the line `@AGENTS.md`.

- Never edit a generated `.md` file directly.
- Always edit the corresponding `.tmt` file under `tmtroot/`.
- Run `just docs` (or `sisho fix .`) to regenerate the Markdown artifacts, and verify with `just docs-check`.
- Direct edits to generated `.md` files will be overwritten and will fail lint checks.

## What this is

`fs-catasto` (CLI binary `catasto`) is the filesystem schema, directory rule, media manifest, and lockfile tool in the Tomet ecosystem.

Read `CONTRIBUTING.md` for the contribution workflow, commit conventions, and verification steps.

## Working against a local tomet

The `tomet-*` crates are git dependencies, pinned by `Cargo.lock`. To try a change in the sibling `../tomet` checkout without a push, point the crates at it in `.cargo/config.toml`, which is gitignored:

```toml
[patch."https://github.com/tomet-lang/tomet.git"]
tomet-ast = { path = "../tomet/crates/tomet-syntax-ast" }
```

## Language

Write code comments and documentation in English. Do not use Japanese in code.

## Environment & Tooling

All commands, builds, and tests must run within the Nix development environment (via `direnv` or `nix develop`). Assume this environment is active and rely on the tools provided by the flake, rather than assuming or relying on global host toolchains.

## Verifying changes

Useful recipes defined in `justfile`:

- `just build`: Builds the workspace.
- `just test`: Runs all workspace tests.
- `just clippy`: Runs clippy across all targets.
- `just docs`: Generates derived documents and API docs via `sisho fix .`.
- `just docs-check`: Verifies all documents and workspace rules via `sisho check .`.

## Task tracking & Workspaces

Before starting implementation on any non-trivial task, create a file under `.agents/tasks/` (one per task, e.g. `.agents/tasks/<short-task-slug>.md`) breaking the work into discrete steps. Update it immediately after each step. Keep it accurate enough that the work can be resumed from that file alone, without the conversation that produced it.

For non-trivial tasks, isolate work in a dedicated Jujutsu workspace under `.agents/workspaces/`:

- Create the workspace before starting edits: `mkdir -p .agents/workspaces && jj workspace add .agents/workspaces/<short-task-slug>`
- Perform edits, builds, and verification within that workspace directory.
- Minor tasks (such as one-off typos or trivial single-file tweaks) may be performed directly in the default workspace.
- When the task is completed and verified, clean up the workspace: `jj workspace forget <short-task-slug>` and remove the directory `rm -rf .agents/workspaces/<short-task-slug>`.

When the task is done, fold anything worth keeping into where it belongs -- a doc comment, a writ entry, a commit message -- and then delete the task file.

## Commits & Version Control (jj)

The repository uses Jujutsu (`jj`) in colocated mode with Git.

- Favor a branchless workflow. Do not create named Git branches for regular agent tasks.
- All commit messages must follow the convention in `CONTRIBUTING.md`: `type(scope): description`.
- Use `jj describe -m "..."` to set commit messages, and `jj new` to advance to subsequent revisions.
- Always pass `--no-pager` to `jj` subcommands (e.g. `jj status --no-pager`, `jj log --no-pager`).
- If `jj status`/`jj diff` shows changes you did not make, treat them as another person's in-progress work, not as drift to clean up. Always confirm with the user before reverting, restoring, or overwriting them.
- Never commit automatically or on your own initiative. Always ask and get explicit confirmation from the user before finalizing commits or descriptions.
- Do not put `Claude-Session:` or `Co-Authored-By: Claude` trailers in commit messages.
- Never push to a remote without being asked.

