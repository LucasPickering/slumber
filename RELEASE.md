# Release Process

It's easy!

- Make sure `CHANGELOG.md` has the latest release notes under `[Unreleased] - ReleaseDate`
- Regenerate all GIFs with `mise run gifs` (and commit changes)
  - Look at the GIFs and make sure they're correct!
- `cargo release --workspace <major|minor|patch>`
  - If it looks good, add `--execute`

Everything else is automated :)
