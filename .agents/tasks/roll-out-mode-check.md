# Roll out the executable-mode check (`7863f87`) to the repositories that use catasto

Found 2026-11-06 while following tomet main in sisho-desk. Not started. Nothing is broken today: no repository has taken a catasto newer than `7863f87` into its dev shell yet.

Not needed for following tomet Phase 1: the tomet follow-up (2026-11-06/07) was done with the catasto each repository already pinned. The one place it shows: `sisho`'s `flake.lock` holds `catasto` at `7314d1f6` (no mode check) until this rollout is done, so `nix flake update catasto` must wait.

## What happens

`catasto check` since `7863f87` ("preserve and verify git executable mode") reports a managed file whose executable bit differs from the lock. The lock records `"mode": "executable"` only for files that are `100755` in the git source; absence means a normal file.

In sisho-desk, `.editorconfig` is `100755` on disk and in git, the lock has no mode (the source `commonfiles` `base/.editorconfig` is a normal file), and the new catasto says:

```
catasto: .editorconfig: modified: differs from the locked sha256 (`catasto sync` restores it; `catasto lock` keeps it, for a local source)
```

Measured with `nix flake update catasto` (fs-catasto `f76eecc5`) in sisho-desk's workspace, `just docs-check` fails at the `catasto check` step. The sha256 in the lock equals the file's sha256 (`2640a0cd...`), so the message is the mode, not the content.

## Why it matters

17 repositories have a `catasto.lock` (fs-catasto, immermemo, jobserver-gate, oneline, ordiola, sisho-desk, sisho, stela-shelf, tandr, telaion, tomet-cms, tomet-corpus, tomet-shelf, tomet-wiki, tomet, tsuzure-wiki, twrit, vocabularies). Counted by `ls -l */.editorconfig` in the project root, 10 have `-rwxr-xr-x` (arbor-florifera, which has no lock, too) and immermemo has `-rw-r--r--`. Every one of them that is executable on disk will fail `catasto check` the day its `flake.lock` takes a catasto at or after `7863f87`. `sisho`'s `just update-tools` updates catasto together with the other tools, so it would do this to sisho and to anything that follows sisho's pin.

## To decide

- [ ] The fix per repository is `catasto sync` (no `--update`: it reproduces the locked commit, restoring the mode without the network if the commit is cached), then committing the mode change. Confirm that on one repository first with `catasto sync --dry-run`, and that it changes nothing but the executable bit; `sync --update` would also move `rev: main` to today's commonfiles (which has a new vault layer) and must not be mixed in.
- [ ] Why are 10 repositories' `.editorconfig` executable at all? Look at how the first commit of each got the bit (`git log --stat --diff-filter=A -- .editorconfig`); if it was a copy from an executable file, other managed files may be wrong the same way, which `catasto check` will list.
- [ ] Order: do the re-sync in each repository before (or in the same change as) the `flake.lock` update that takes the new catasto. Put the mode change in its own commit (`chore(deps): ...`), not inside a tomet-follow commit.
- [ ] Whether a lock written before `7863f87` (no `mode` anywhere) should be told apart from "this file is a normal file" is catasto's own question: the lock has `version: 1` both before and after. If an old lock is to be refused with "re-lock with `catasto sync`" instead of a per-file `modified`, that is a change here, not a rollout step.

## Destination when done

The rollout state goes in `bearings/initiatives/commonfiles-rollout.tmt` (one row per repository); the lock-version question goes in catasto's lockfile doc comment or its README, wherever it is decided. Delete this file.

## Update 2026-11-07

`sync` now sets the bits to the source's in both directions, whatever the old lock said (`066da470`), and an entry can
declare `executable:` (`14c884e7`). So the fix per repository above also removes a bit the source does not have. The
re-run over `project-tomet/` is tracked in `executable-mode-followups.md`.
