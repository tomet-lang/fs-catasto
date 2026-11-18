# catasto usable in other repositories: what is left

> Cross-repository coordination: `bearings/initiatives/catasto-and-sisho.tmt` (the table and the order live there; the work stays here).

Landed 2026-10-24 (fs-catasto `c7af7621`, sisho docs and pin): `init`, `add`, `remove`, `upstream add|remove|list`, `list [--json]`,
`sync --only`, the fix hint in each `check` line, the old-manifest-name error, `-C/--workspace`. The design and the how-to are in
`tmtroot/readme.tmt` ("Usage", "Getting Started"); the edits-as-text approach is in `crates/catasto-core/src/edit.rs`.

## Left (none is needed for the goal)

- [ ] `catasto check --json` (a kind and a hint per file), so tools can say more than "exit 1".
- [ ] `catasto sync --missing` (only files that are missing or unlocked, never a modified one): the safe half of syncing, for `sisho fix`
  (see `sisho/.agents/tasks/catasto-through-sisho.md`).
- [ ] `catasto upstream set NAME --rev R` to move a place to another tag without editing the file.
- [ ] A hand-written manifest with a value that starts on the line after its `key:` is read by the scanner, but no test covers it.
- [ ] The `catasto-next.md` list (rules by path, media) is untouched.
