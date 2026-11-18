# Executable mode: what is left after `--all` discovery, the sync fix and `executable:`

Landed 2026-11-07 on `main` (pushed): `4653bc4b` (`--all` finds every `catasto.ctmt`, root and nested), `066da470`
(`sync` sets the executable bits to the source's, whatever the old lock said), `14c884e7` (`executable: true|false` in an
entry). The details are in those commit messages and in `tmtroot/readme.tmt`.

## Left

- [ ] Re-run the rollout over `project-tomet/` with a build of `main`: `catasto sync --all .` (no `--update` unless
      wanted). 16 repositories were synced earlier with the buggy build and are uncommitted in their working copies;
      `check --all` then showed 7 failing for the exec bit (`.envrc` `777` against a `100644` source; `.gitattributes` in
      tomet-corpus and twrit). Three repositories hold local work in managed files (fs-catasto, immermemo,
      tsuzure-wiki), which `sync` overwrites: look at them first. Do not commit or push in the repositories without
      asking once for all of them.
- [ ] `catasto add --executable/--no-executable`, and `catasto list` showing the mode. Hand-editing works today.
- [ ] Open: should a local source follow the source file's own bit when `executable` is absent, like git does? It changes
      what existing local-source entries do (non-executable today), so it was left alone.
- [ ] `project-tomet/catasto.ctmt` is empty and is now one of the 19 workspaces `--all` finds (it passes). Keep it or
      remove it: the user's call.
- [ ] The repositories that want a script bit the source does not have (`.envrc` in oneline, tomet, tomet-shelf, twrit)
      should say `executable: ...` in their own manifest if they differ from the source: decide per repository.
