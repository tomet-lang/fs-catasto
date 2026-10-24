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
- Settings that belong to no path go in the header of `catasto.motm` as a
  `::name` declaration; `::upstreams` is the first. A setting like
  editorconfig's `root` (stop looking at parent manifests) would be one of
  those, not a key among the paths.

Delete this file when these are taken up as their own tasks.
