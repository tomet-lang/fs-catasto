# `catasto check`: warn about a dangling placed symlink

Follow-up of the git symlink work (`feat: follow or keep symlinks in git sources`). Self-contained; read `AGENTS.md` and `CONTRIBUTING.md` first.

## Why

An entry with `symlink: keep` places the upstream link text verbatim. The text is relative to where the link was upstream, so in a workspace with
another layout it may point at nothing. catasto deliberately does not call that a violation: the link is what upstream has, and `check` is about
drift from the lock. But the user should be told, because a dangling link breaks whatever reads it, later and far from catasto.

Today `catasto_core::check` returns `CheckReport { root, violations }` (`crates/catasto-core/src/check.rs`) and `app/src/commands/check.rs`
prints each violation as `catasto: <path>: <kind>` and exits 1 when there is one. There is no channel for something that is not a violation.

## What

- A warning channel next to the violations: `CheckReport` gets `warnings: Vec<Warning>` (shape is yours; keep `is_ok()` meaning "no violations", so
  a warning never changes the exit code).
- One warning kind for now: a kept link whose target does not resolve. A path counts as dangling when the destination is a symlink
  (`symlink_metadata`) and following it fails (`metadata` is NotFound, or the chain loops / is too deep: say "cannot be resolved" then).
  Only entries that place a link are looked at: those with `symlink: keep`, and the manifest-declared `{ link: "text" }` entries
  (`.agents/tasks/link-entry.md`; if that order has not landed yet, cover `keep` now and leave a note there). Only when the destination is a
  symlink (a missing or replaced destination is already a violation). For a `link` entry the case is common and legitimate: the target may be
  generated after `check` runs, which is why this is a warning and never a violation.
- The app prints `catasto: warning: <path>: dangling symlink -> <link text>` on stderr, after the violations, and still exits 0 when there are
  only warnings. `catasto list` does not change.
- Keep the type open for more warning kinds, but do not invent any.

## Coordinate

`.agents/tasks/usable-in-other-repos.md` lists `catasto check --json` ("a kind and a hint per file"). If that lands first, warnings go into the
JSON as their own array; if this lands first, leave a note there. Do not do both here.

## Tests

`check.rs`: a kept link to an existing file (no warning), to nothing (one warning, no violation, `is_ok()` true), a link loop (warning), a
default entry whose destination is a dangling link (a violation `modified`, no warning), a missing destination (violation only).
`app/tests/cli.rs`: warning text, stderr, exit code 0.

## Docs

`tmtroot/readme.tmt`, in the `check` section: what a warning is and that it never fails the run. Then `just docs`.

## Acceptance

`just build`, `just test`, `just clippy`, `just docs-check` pass in the nix shell.
