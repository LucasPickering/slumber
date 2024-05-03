# Contribution Guidelines

First off, thank you for considering contributing to Slumber.

If your contribution is not straightforward, please first discuss the change you wish to make by creating a new issue before making the change. For more detailed discussions, consider joining the [Discord server](https://discord.gg/G9RbbUsj) for faster communication.

## Reporting Issues

Before reporting an issue on the [issue tracker](https://github.com/LucasPickering/slumber), please check that it has not already been reported by searching for some related keywords.

## Pull Requests

All contributions are welcome. Please include as many details as possible in your PR description to help the reviewer (follow the provided template). Make sure to highlight changes which may need additional attention or you are uncertain about. Any idea with a large scale impact on the crate or its users should ideally be discussed in a "Feature Request" issue beforehand.

### Keep PRs Small, intentional and Focused

Try to do one pull request per change. The time taken to review a PR grows exponential with the size of the change. Small focused PRs will generally be much more faster to review. PRs that include both refactoring (or reformatting) with actual changes are more difficult to review as every line of the change becomes a place where a bug may have been introduced. Consider splitting refactoring/reformatting changes into a separate PR from those that make a behavioral change, as the tests help guarantee that the behavior is unchanged.

### Code Formatting

We use Rustfmt for formatting. Due to a couple very useful options only being unstable, formatting must be run on the Rust nightly toolchain. You can do so with:

```sh
rustup install nightly
cargo +nightly fmt
```

Generally the nightly version doesn't matter, but if you want to make sure you're using the same version as the CI, you can check which version is used in [test.yml](https://github.com/LucasPickering/slumber/blob/master/.github/workflows/test.yml).

### Git

- Use detailed commit messages. Commit messages form an important historical log for the repository. In your commit message, please include a description of **all** changes, and a link to the relevant GitHub issue (the issue number is enough, e.g. "Closes #100").
- _Generally_ each PR should be a single commit. If you have a major change (e.g. a rename or refactor) that clutters the diff, consider putting that in a separate PR rather than just a separate commit.
- Subsequent commits on a PR (e.g. changes made in response to feedback) should be **separate commits**. This makes it much easier to review changes. We'll squash into a single commit before merging.
- Git history must be linear, meaning you should do `git rebase master` instead of `git merge master` on your branch. If you don't know how to use `rebase`, there are plenty of tutorials online.

## Implementation Guidelines

### Prerequisites

You'll need the following tools to build and run Slumber locally:

- [rustup](https://rustup.rs/)
- [watchexec-cli](https://crates.io/crates/watchexec-cli)
  - Not required, but useful for automatically re-running after changes
- [oranda](https://opensource.axo.dev/oranda/artifacts/)
  - Only required if making documentation changes

That's it!

### Setup

- [Fork the repo](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/fork-a-repo)
- Clone your fork
- `cd slumber`
- Run `./run.sh`
  - This will install the appropriate Rust/Cargo toolchain if you don't have it already
  - If you don't have `watchexec-cli` installed, you can just run `cargo run` instead

### Tests

The [test coverage](https://app.codecov.io/gh/ratatui-org/ratatui) of the crate is reasonably good, but this can always be improved. Focus on keeping the tests simple and obvious and write unit tests for all new or modified code. Slumber uses [rstest](https://docs.rs/rstest/latest/rstest/) and [factori](https://docs.rs/factori/latest/factori/) to make testing easier. Try to follow existing test patterns. Some general rules to follow:

- Prefer [parameterized tests](https://docs.rs/rstest/latest/rstest/#creating-parametrized-tests) over a single long test that checks a lot of cases. This makes it easier to isolate failing test cases.
- Use existing factories and functions from [test_util.rs](https://github.com/LucasPickering/slumber/blob/master/src/test_util.rs)
- If you add any new functions or trait implementations specifically for testing, make sure they are gated by `#[cfg(test)]` or `#[cfg_attr(test)]`

If you're not sure how to write tests for your change, feel free to post your PR without tests, or with incomplete tests, and a maintainer can help guide you on how to write them.

If an area that you're making a change in an area that is not already tested, it is helpful but not required to write tests for existing behavior.

### Documentation

Slumber's documentation is written using [mdBook](https://rust-lang.github.io/mdBook/), and generating by [Oranda](https://opensource.axo.dev/oranda/). Documentation source is located in [docs/src](https://github.com/LucasPickering/slumber/tree/master/docs/src).

To build documentation locally, use:

```
oranda dev
```

#### Writing Docs

- Any new file added to the book must be listed in `SUMMARY.md`
- All fields, types, etc. in the collection (`slumber.yml`) or config (`config.yml`) files **must** be documented
- Larger features should be documented via a section in the user guide
- CLI commands should be documented under the the CLI section of the API reference. This documentation does **not** need to be exhaustive, as we already have `--help`.
- UI features do _not_ need to be documented. If a UI needs documentation, it's probably not good anyway :)

When writing docs, try to follow existing style/patterns of related pages. User Guide pages follow a tutorial-like structure, while API pages should include a description of available fields/variants, as well as examples.

### Use of `unsafe`

Don't!

## Continuous Integration

CI runs on Github Actions. Tests, linting, and documentation will be checked automatically.
