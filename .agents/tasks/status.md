# Work order: `catasto status` -- what is managed, what is not, how each stands

Ordered 2026-10-25. Self-contained. Read `AGENTS.md` and `CONTRIBUTING.md` first; they bind you (nix dev shell, `just` recipes, `.tmt` is the source of
every `.md`, English only, `jj --no-pager`, no commit without asking, no push, do not touch changes you did not make). Delete this file when done.

Independent of the five `add-*` / `diff` / `sync-dry-run` orders; it may be done in parallel with them but touches `catasto-core`'s public API, so rebase
on whatever has landed.

## 1. Why

`catasto check` says what has drifted and, on purpose, nothing about files that are not declared or locked (`check.rs`: "unmanaged and never reported").
Someone looking at a tree cannot see which files catasto governs, which it does not, and how the governed ones stand. A file explorer (sisho-desk's, for
one) is where people look at a tree, so **catasto does not ship a GUI**: it ships the classification, as a library function and as JSON, and a plain
text view for the terminal. The explorer decorates files from that data.

## 2. Decisions (taken by the orderer; do not reopen)

1. **Core function** in `catasto-core`: classify every path of the workspace tree. One entry per file (and per symlink, not followed), with:
   - `class`: `managed` | `unmanaged` | `ignored`;
   - for `managed`: `status`: `ok` | `modified` | `missing` | `stale` | `unlocked` | `orphan` (the existing `ViolationKind` plus `ok`; reuse it, do not
     duplicate the logic of `check` -- `check` and `status` must agree on every managed file, and a test asserts it);
   - `violations`: a list, **always present, empty for now**. Directory and naming rules do not exist yet (`.agents/tasks/catasto-next.md`); this is the
     reserved place for them so the JSON shape does not change when they arrive. Do not invent rules.
2. **`ignored`** means: matched by the workspace's `.gitignore` files (nested ones, global excludes not included) or `.git/info/exclude`, or inside `.git`
   / `.jj`. Use the `ignore` crate (it is the one ripgrep uses) unless it cannot be vendored for the nix build; report the choice. A managed file that is
   also ignored is `managed` (the manifest is the stronger statement).
3. `catasto.ctmt` and `catasto.lock` are left out of the report: they are catasto's own, neither governed nor foreign.
4. A missing managed file has no path on disk; it is still listed (`class: managed`, `status: missing`). Directories are not entries; an empty directory
   does not appear. Paths are relative to the workspace root, `/`-separated, sorted.
5. `status` never touches the network, like `check`. It reads the same things `check` reads.
6. **CLI:** `catasto status [PATH] [--json] [--all]`.
   - Text (default): only what needs a person's attention, grouped, one line per file, like `git status`: managed files that are not `ok`, then
     unmanaged files. `ignored` and `ok` files are hidden unless `--all`. Collapse a directory whose files are all unmanaged into `dir/` (a count in
     parentheses). Exit **0** always (it reports; `check` is the verdict).
   - `--json`: the full classification, nothing hidden, one document on stdout:
     `{ "version": 1, "files": [ { "path": "...", "class": "...", "status": "...", "violations": [] } ] }` (`status` only for `managed`).
     **`version` is part of the contract**: bump it for any breaking change of this shape, and say in the doc comment that consumers must check it. An
     unknown future field must not break a consumer that ignores unknown fields; document that adding fields is not a version bump.
7. **No GUI, no tree rendering, no colors beyond what the other commands use.** Presentation belongs to explorers; do not add ratatui or similar.

## 3. Tests

`catasto-core`: a fixture tree with one file in each state (`ok`, `modified`, `missing`, `stale`, `unlocked`, `orphan`), an unmanaged file, an ignored file
(via `.gitignore`), a managed-and-ignored file, a nested `.gitignore`, a symlink, and `.git`/`.jj` directories; assert the whole classification. For every
file the status agrees with `check` (property: the set of non-ok managed files equals the set of paths in the `check` report). `app/tests/`: text output
for the fixture (grouping, collapse, `--all`), `--json` parses and carries `version: 1` and `violations: []` on every entry, exit code 0 on a drifted
tree.

## 4. Acceptance

`just build`, `just test`, `just clippy`, `just docs-check` pass. `--help` and `tmtroot/readme.tmt` (Usage, plus a short section on the JSON contract and
its version) updated, docs regenerated with `just docs`. Do not edit `tmtroot/spec/**` (someone's staged work in progress).
