# `catasto init` writes a 16-line comment into every new manifest

Moved out of tomet's `shared-files-template.md` (2026-11-06; the cross-repository part is in `bearings/initiatives/commonfiles-rollout.tmt`).

`catasto init` writes a 16-line comment block (`SKELETON` in `crates/catasto-core/src/manage.rs`) at the top of every new manifest.
Nobody asked for it, and it stays at the top after the first `add`. In the repositories that took commonfiles it was replaced by hand
with a 2-line header.

- [ ] Decide: shorten it to one line, or write nothing. The user decides; do not pick.
- [ ] Then change `SKELETON` and the tests that expect it.

Delete this file when that is done.
