# Release Process

It's easy!

- Make sure `CHANGELOG.md` has the latest release notes under `[Unreleased] - ReleaseDate`
- Regenerate the demo gif with `./demo.sh` (and commit changes)
- `cargo release <major|minor|patch>`
  - If it looks good, add `--execute`

Everything else is automated :)
