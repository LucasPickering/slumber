# Release Process

It's easy!

- Make sure `CHANGELOG.md` has the latest release notes under `[Unreleased] - ReleaseDate`
- Regenerate all GIFs with `./gifs.py` (and commit changes)
  - Look at the GIFs and make sure they're correct!
- `cargo release --workspace <major|minor|patch>`
  - If it looks good, add `--execute`
- After the release commit is pushed, the [Web action needs to be run manually](https://github.com/LucasPickering/slumber/actions/workflows/web.yml) to update the website

Everything else is automated :)
