# catasto: what comes after the manifest

The manifest is path-first motem (`catasto.motm`) and the lock is JSON. These
are left open.

- Rules by path: naming and prefix rules, directory rules (what may live in a
  directory, what is required). Keys are globs or a trailing `/`; later entries
  override earlier ones, as in editorconfig, so the manifest needs an ordered
  container. A file entry (an exact path) may carry `source`; a rule may not.
  The entry type is a struct with one field today.
- Media: a file entry with extra metadata (license, origin), not a second
  section.
- A manifest that holds only comments is rejected with `expected a value`:
  motem has no standalone-document entry point where an empty document is the
  empty map. Fix it in motem (tomet's `.agents/tasks/mtm-standalone-file.md`).
- `nix build` of the package has not been run since `Cargo.lock` gained a git
  dependency on `tomet-lang/tomet`.
- Settings that belong to no path have no place yet (editorconfig's `root`).
  Decide when the first one is needed.

Delete this file when these are taken up as their own tasks.
