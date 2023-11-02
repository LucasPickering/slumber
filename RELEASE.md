# Release Process

It's easy!

- Add release notes to `CHANGELOG.md`
  - Add a new header in the format `## [x.y.z] - yyyy-mm-dd`
- `cargo release <major|minor|patch>`
  - If it looks good, add `--execute`

Everything else is automated :)
