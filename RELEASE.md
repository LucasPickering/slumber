# Release Process

It's easy!

- Make sure `CHANGELOG.md` has the latest release notes under `[Unreleased] - ReleaseDate`
- Regenerate all GIFs with `mise run gifs` (and commit changes)
  - Look at the GIFs and make sure they're correct!
  - If you're really absolutely sure the GIFs haven't changed (e.g. for a small bug fix release), you can skip this check with `SLUMBER_SKIP_GIF_CHECK=true cargo release ...`
- `cargo release --workspace <major|minor|patch>`
  - If it looks good, add `--execute`

Everything else is automated :)
