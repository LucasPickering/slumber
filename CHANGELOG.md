# Changelog

All user-facing changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - ReleaseDate

### Breaking

- Don't store CLI requests in history
- Simplify display for `slumber request`
  - The flags `--status`, `--headers` and `--no-body` have been removed in favor of a single `--verbose` flag
- Remove DB migration to upgrade from the pre-1.8.0 DB format
  - This only impacts users upgrading to 3.0.0 from versions _before_ 1.8.0. You'll need to upgrade to an intermediate version first. If you install 3.0.0 and try to start it, you'll see an error message explaining how to fix it.
  - See [#306](https://github.com/LucasPickering/slumber/issues/306) for more info

### Added

- Replace JSONPath querying with general purpose shell commands for querying response bodies
  - Now you can access any CLI tools you want for transforming response bodies, such as `jq` or `grep`
  - By default, commands are executed via `sh` (or `cmd` on Windows), but this is configured via the [`commands.shell` field](https://slumber.lucaspickering.me/book/api/configuration/index.html)
- Add `slumber history` subcommand. Currently it has two operations:
  - `slumber history list` lists all stored requests for a recipe
  - `slumber history get` prints a specific request/response
- Add `--output` flag to `slumber request` to control where the response body is written to

### Added

- Add REST Importer for VSCode and Jetbrains [#122](https://github.com/LucasPickering/slumber/issues/122) (thanks @benfaerber)

### Changed

- Update [editor-command](https://crates.io/crates/editor-command), which replaces [shellish_parse](https://crates.io/crates/shellish_parse) with [shell-words](https://crates.io/crates/shell-words) for editor and pager command parsing
  - There should be no impact to users
- Don't show "Loaded collection from ..." notification on initial load

## [2.4.0] - 2024-12-27

### Added

- Add filter box to the recipe list
  - This behavior is not necessarily final. Please leave feedback if you think it could be improved.

### Changed

- Wrap long error messages in response pane
- Include data path in config/collection deserialization errors
  - This should make errors much less cryptic and frustrating
- Improve UX of query text box
  - The query is now auto-applied when changed (with a 500ms debounce), and drops focus on the text box when Enter is pressed
- Refactor UI event handling logic
  - This shouldn't have any noticable impact on the user, but if you notice any bugs please open an issue
- Include request duration in History modal
- Rename `viewer` config field to `pager`
  - The old field name `viewer` is still supported for backward compatibility, but the docs have been updated to suggest the newer name instead
- Load pager command from the `PAGER` environment variable if available, similar to the `EDITOR` environment variable

### Fixed

- Don't show request cancellation dialog if the selected request isn't building/loading

## [2.3.0] - 2024-11-11

### Added

- Add "View Body" action to response bodies, to open a body in an external viewer such as `less` or `fx` [#404](https://github.com/LucasPickering/slumber/issues/404)
  - By default `less` is used. You can customize this with the [`viewer` config field](https://slumber.lucaspickering.me/book/api/configuration/editor.html)

### Changed

- Preserve key order of objects in JSON responses [#405](https://github.com/LucasPickering/slumber/issues/405)

### Fixed

- Fixed `ignore_certificate_hosts` and `large_body_size` fields not being loaded from config
- Improve performance of large response bodies [#356](https://github.com/LucasPickering/slumber/issues/356)
  - This includes disabling prettyification and syntax highlighting on bodies over 1 MB (this size is configurable, via the `large_body_size` [config field](https://slumber.lucaspickering.me/book/api/configuration/index.html))
  - Loading a large response body should no longer cause the UI to freeze or low framerate

## [2.2.0] - 2024-10-22

### Added

- Add shell completions, accessed by enabling the `COMPLETE` environment variable
  - For example, adding `COMPLETE=fish slumber | source` to your `fish.config` will enable completions for fish
  - [See docs](https://slumber.lucaspickering.me/book/troubleshooting/shell_completions.html) for more info and a list of supported shells
- Add `slumber gen` alias to `--help` documentation

### Fixed

- Fix error loading requests with empty header values from history [#400](https://github.com/LucasPickering/slumber/issues/400)
- Fix input bindings involving `shift` and a character (e.g. `shift g`) [#401](https://github.com/LucasPickering/slumber/issues/401)

## [2.1.0] - 2024-09-27

### Added

- Use `SLUMBER_CONFIG_PATH` to customize configuration (_not_ collection) file path [#370](https://github.com/LucasPickering/slumber/issues/370)
- Add a dynamic variant to `!select` chain type, allowing your collection to present a list of values driven from the output of another chain. (thanks @anussel5559)
  - [See docs for more](https://slumber.lucaspickering.me/book/api/request_collection/chain_source.html#select)
- Cancel in-flight requests with the `cancel` action (bound to escape by default)
- Add `slumber new` subcommand to generate new collection files [#376](https://github.com/LucasPickering/slumber/issues/376)
- Add `default` field to profiles
  - When using the CLI, the `--profile` argument can be omitted to use the default profile
- Reset edited recipe values to their default using `z`
  - You can [customize the key](https://slumber.lucaspickering.me/book/api/configuration/input_bindings.html) to whatever you want
- Add `selector_mode` field to chains, to control how single vs multiple results from a JSONPath selector are handled
  - Previously, if a selector returned multiple results, an error was returned. Now, the result list will be rendered as a JSON array. To return to the previous behavior, set `selector_mode: single` in your chain.
  - [See docs for more](https://slumber.lucaspickering.me/book/api/request_collection/chain.html#selector-mode)

### Changed

- Update file locations to adhere to XDG spec on Linux [#371](https://github.com/LucasPickering/slumber/issues/371)
  - Move config file to [config dir](https://docs.rs/dirs/latest/dirs/fn.config_dir.html), which remains the same on MacOS/Windows but changes on Linux. For backward compatibility, the previous path ([data dir](https://docs.rs/dirs/latest/dirs/fn.data_dir.html)) will be checked and used if present
  - Move log files to [state dir](https://docs.rs/dirs/latest/dirs/fn.state_dir.html) on Linux and [cache dir](https://docs.rs/dirs/latest/dirs/fn.cache_dir.html) on MacOS/Windows
  - Database file remains in [data dir](https://docs.rs/dirs/latest/dirs/fn.data_dir.html) on all platforms
- Create config file on startup if it doesn't exist
- If config file fails to load during TUI startup, display an error and fall back to the default, rather than crashing
- De-deprecate `{{env.VARIABLE}}` template sources
  - They'll remain as a simpler alternative to `!env` chains

### Fixed

- Updated the Configuration docs to remove the non-existent `slumber show dir` command (thanks @SVendittelli)
- Retain all request history when collection file is reloaded
  - Previously, pending and failed requests were lost on reload within a single session. These will still be lost when a session is exited.
- Fix serialization of query parameter lists
- Don't update UI for useless events (e.g. cursor moves)

## [2.0.0] - 2024-09-06

2.0 is headlined by a highly requested feature: one-off edits to recipes! If you need to tweak a query parameter or edit a body, but don't want to modify your collection file, you can now highlight the value in question and hit `e` to modify it. The override will be retained until you modify the collection file or exit Slumber, at which point it will revert to its original value.

Aside from the major new feature, there is one breaking change to the escape syntax of templates. The old backslash-based syntax was fraught with edge cases and unpredictable behavior. This new syntax is simpler to use, simpler to implement, and much more bulletproof. This syntax was rare to use to begin with, so **most people will be unimpacted by this change.**

Here's the full list of changes:

### Breaking

- Replace backslash escape sequence with a simpler scheme based on `_`
  - For example, previously a key would be escaped as `\{{`. This introduced complexities around how to handle additional backslashes, and also required doubling up backslashes in YAML
  - The new equivalent would be `{_{`, which parses as `{{`
  - The goal of this change is to make escaping behavior simpler and more consistent
  - For more info on the new behavior, [see the docs](https://slumber.lucaspickering.me/book/api/request_collection/template.html#escape-sequences)
- Remove `--log` CLI argument
  - See note on log files in Changed section for why this is no longer necessary

### Added

- Edit recipe values (query params, headers, etc.) in the TUI to provide one-off values
  - Press `e` on any value you want to edit (you can [customize the key](https://slumber.lucaspickering.me/book/api/configuration/input_bindings.html))
- Add `editor` field to the config, allowing you to customize what editor Slumber opens for in-app editing
  - [See docs for more](https://slumber.lucaspickering.me/book/api/configuration/editor.html)
- Add `!select` chain type, allowing your collection to prompt the user to select a value from a static list (thanks @anussel5559)
  - [See docs for more](https://slumber.lucaspickering.me/book/api/request_collection/chain_source.html#select)

### Changed

- `!json` bodies are now prettified when sent to the server
- Use `vim` as default editor if none is configured
- Move logs back to a shared file
  - They had been split into one file per session, which made them hard to find
  - The file is now eventually deleted once it exceeds a certain size

### Fixed

- Fix basic auth being label as bearer auth in Recipe Authentication pane
- Use correct binding for `search` action in the placeholder of the response filter textbox
  - Previously it was hardcoded to display the default of `/`
- Fix response body filter not applying on new responses
- Support quoted arguments in editor commands
- Fix certain UI values not persisting correctly
- Propagate unconsumed key events from text boxes
  - E.g. F5 will now refresh the collection while a text box is in focus
- Redraw TUI when terminal is resized
- Clamp text window scroll state when window is resized or text changes
- Fix extraneous input events when exiting Vim [#351](https://github.com/LucasPickering/slumber/issues/351)
- Improve performance and fix crashes when handling large request/response bodies [#356](https://github.com/LucasPickering/slumber/issues/356)
  - Further improvements for large bodies will be coming in the future

## [1.8.1] - 2024-08-11

This release is focused on improving rendering performance. The TUI should generally feel more polished and responsive when working with large bodies, and CPU usage will be much lower.

### Added

- Add `debug` configuration field, to enable developer information

### Fixed

- Reduce CPU usage while idling
  - Previously, Slumber would re-render every 250ms while idling, which could lead to high CPU usage, depending on what's on the screen. Now it will only update when changes occur, meaning idle CPU usage will be nearly 0
- Fix backlogged events when renders are slow
  - If renders are being particular slow, it was previously possible for input events (e.g. repeated scrolling events) to occur faster than the UI could keep up. This would lead to "lock out" behavior, where you'd stop scrolling and it'd take a while for the UI to catch up.
  - Now, the TUI will skip draws as necessary to keep up with the input queue. In practice the skipping should be hard to notice as it only occurs during rapid TUI movements anyway.
- Improve rendering performance for large bodies and syntax highlighting
- Fix incorrect decoration in folder tree visualization

## [1.8.0] - 2024-08-09

The highlight (no pun intended) of this release is syntax highlighting. Beyond that, the release contains a variety of small fixes and improvements.

### Added

- Add syntax highlighting to recipe, request, and response display [#264](https://github.com/LucasPickering/slumber/issues/264)

### Changed

- Change layout of internal database for request and UI state storage
  - This _shouldn't_ have any user impact, it's just a technical improvement. If you notice any issues such as missing or incorrect request history, please [let me know](https://github.com/LucasPickering/slumber/issues/new?assignees=&labels=bug&projects=&template=bug_report.md)
- Upgrade to Rust 1.80
- Disable unavailable menu actions [#222](https://github.com/LucasPickering/slumber/issues/222)
- Support template for header names in the `section` field of `!request` chains
- Expand `~` to the home directory in `!file` chain sources and when saving response body as a file
- Ignore key events with additional key modifiers
  - For example, an action bound to `w` will no longer match `ctrl w`
- Actions can now be unbound by specifying an empty binding
  - For example, binding `submit: []` will make the submit action inaccessible

### Fixed

- Fix `cargo install slumber` when not using `--locked`
- Don't type in text boxes when modifiers keys (other than shift) are enabled
  - Should mitigate some potential confusing behavior when using terminal key sequences
- Query parameter and header toggle rows no longer lose their state when switching profiles

## [1.7.0] - 2024-07-22

This release focuses on minor fixes and improvements. There are no new major features or added functionality.

### Added

- Add global `--log` argument to CLI, to print the log file being used for that invocation

### Changed

- Checkbox row state and folder expand/collapse state are now toggled via the spacebar instead of enter
  - Enter now sends a request from anywhere. While this change may be annoying, it will hopefully be more intuitive in the long run.
  - This can be rebound ([see docs](https://slumber.lucaspickering.me/book/api/configuration/input_bindings.html))
- Show folder tree in recipe pane when a folder is selected
- Don't exit body filter text box on Enter [#270](https://github.com/LucasPickering/slumber/issues/270)
- Show elapsed time for failed requests (e.g. in case of network error)

### Fixes

- Fix latest request not being pre-selected correctly if it's not a successful response
- Detect infinite loops in chain configuration templates
- Duplicated chains in a recipe will only be rendered once [#118](https://github.com/LucasPickering/slumber/issues/118)
- Never trigger chained requests when rendering template previews in the TUI
- Use a different log file for each session [#61](https://github.com/LucasPickering/slumber/issues/61)

## [1.6.0] - 2024-07-07

### Added

- Initial support for importing collections from an OpenAPIv3 specification [#106](https://github.com/LucasPickering/slumber/issues/106)
  - Currently only OpenAPI 3.0 (not 3.1) is supported. Please try this out and give feedback if anything doesn't work.

### Changed

- Allow escaping keys in templates [#149](https://github.com/LucasPickering/slumber/issues/149)
  - While this is technically a breaking change, this is not a major version bump because it's extremely unlikely that this will break anything in practice for a user
  - [See docs](https://slumber.lucaspickering.me/book/api/request_collection/template.html#escape-sequences)

### Fixed

- Support TLS certificates in native certificate store [#275](https://github.com/LucasPickering/slumber/issues/275)

## [1.5.0] - 2024-06-17

### Added

- Add `!env` chain source, for loading environment variables
  - This is intended to replace the existing `{{env.VARIABLE}}` syntax, which is now deprecated and will be removed in the future

### Changed

- "Edit Collection" action now uses the editor set in `$VISUAL`/`$EDITOR` instead of whatever editor you have set as default for `.yaml`/`.yml` files [#262](https://github.com/LucasPickering/slumber/issues/262)
  - In most cases this means you'll now get `vim` instead of VSCode or another GUI editor
  - Closing the editor will return you to Slumber, meaning you can stay in the terminal the entire time

### Fixed

- Environment variables in `{{env.VARIABLE}}` templates are now loaded as strings according to the OS encoding, as opposed to always being decoded as UTF-8

## [1.4.0] - 2024-06-11

### Added

- Structured bodies can now be defined with tags on the `body` field of a recipe, making it more convenient to construct bodies of common types. Supported types are:
  - `!json` [#242](https://github.com/LucasPickering/slumber/issues/242)
  - `!form_urlencoded` [#244](https://github.com/LucasPickering/slumber/issues/244)
  - `!form_multipart` [#243](https://github.com/LucasPickering/slumber/issues/243)
  - [See docs](https://slumber.lucaspickering.me/book/api/request_collection/recipe_body.html) for usage instructions
- Support multiple instances of the same query param [#245](https://github.com/LucasPickering/slumber/issues/245) (@maksimowiczm)
  - Query params can now be defined as a list of `<param>=<value>` entries
  - [See docs](https://slumber.lucaspickering.me/book/api/request_collection/query_parameters.html)
- Templates can now render binary values in certain contexts
  - [See docs](https://slumber.lucaspickering.me/book/user_guide/templates.html#binary-templates)

### Changed

- When a modal/dialog is open `q` now exits the dialog instead of the entire app
- Upgrade to Rust 1.76

### Fixed

- Fix "Unknown request ID" error showing on startup [#238](https://github.com/LucasPickering/slumber/issues/238)

## [1.3.2] - 2024-05-27

### Changed

- Show "Copy URL", "Copy Body" and "Copy as cURL" actions from the Recipe list [#224](https://github.com/LucasPickering/slumber/issues/224)
  - Previously this was only available in the Recipe detail pane
- Fix Edit Collection action in menu
- Persist response body query text box contents
  - Previously it would reset whenever you made a new request or changed recipes

## [1.3.1] - 2024-05-21

### Fixed

- Fix double key events on Windows [#226](https://github.com/LucasPickering/slumber/issues/226)

## [1.3.0] - 2024-05-17

The biggest feature in this release is the ability to browse request history. Slumber has already had the ability to _track_ history, meaning all your history since you started using it will already be there! In addition, this release contains some UI improvements, as well as some pretty major internal refactors to enable these UI changes. These will also make future UI improvements easier and faster to implement.

### Added

- Request history is now browsable! [#55](https://github.com/LucasPickering/slumber/issues/55)
- Add scrollbars to lists and text windows [#220](https://github.com/LucasPickering/slumber/issues/220)

### Changed

- Merge request & response panes
  - The request pane often isn't needed, so it doesn't deserve top-level space
- Mouse events (e.g. scrolling) are now sent to unfocused elements

## [1.2.1] - 2024-05-11

### Fixed

- Fix profile not being selected on initial startup

## [1.2.0] - 2024-05-10

### Added

- Add `trim` option to chains, to trim leading/trailing whitespace [#153](https://github.com/LucasPickering/slumber/issues/153)
  - [See docs](https://slumber.lucaspickering.me/book/api/request_collection/chain.html#chain-output-trim)

### Changed

- Use colored background for status codes
  - This includes a new theme field, `success_color`
- Improve hierarchy presentation of errors
- Convert profile list into a popup modal

### Fixed

- Exit fullscreen mode when changing panes
- Support scrolling on more lists/tables

## [1.1.0] - 2024-05-05

### Added

- Add `section` field to `!request` chain values, to allow chaining response headers rather than body ([#184](https://github.com/LucasPickering/slumber/issues/184))
- Add action to save response body to file ([#183](https://github.com/LucasPickering/slumber/issues/183))
- Add `theme` field to the config, to configure colors ([#193](https://github.com/LucasPickering/slumber/issues/193))
  - [See docs](https://slumber.lucaspickering.me/book/api/configuration/theme.html) for more info
- Add `stdin` option to command chains ([#190](https://github.com/LucasPickering/slumber/issues/190))

### Changed

- Reduce UI latency under certain scenarios
  - Previously some actions would feel laggy because of an inherent 250ms delay in processing some events
- Search parent directories for collection file ([#194](https://github.com/LucasPickering/slumber/issues/194))
- Use thicker borders for selected pane and modals
- Change default TUI colors to blue and yellow

### Fixed

- Fix Slumber going into zombie mode and CPU spiking to 100% under certain closure scenarios ([#136](https://github.com/LucasPickering/slumber/issues/136))
- Fix historical request/response no loading on first render ([#199](https://github.com/LucasPickering/slumber/issues/199))

## [1.0.1] - 2024-04-27

### Added

- Add two new build targets to releases: `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-musl`

### Fixed

- Fix build on Windows ([#180](https://github.com/LucasPickering/slumber/issues/180))
  - I can't guarantee it _works_ on Windows since I don't have a machine to test on, but it at least compiles now

## [1.0.0] - 2024-04-25

### Breaking

- Rename collection file parameter on all CLI commands from `--collection`/`-c` to `--file`/`-f`
  - The goal here is to be more intuitive/predictable, since `-f` is much more common in similar programs (e.g. docker-compose)

### Added

- Support booleans and numbers for query values ([#141](https://github.com/LucasPickering/slumber/issues/141))
- Add `default` field to `!prompt` chains, which allows setting a pre-populated value for the prompt textbox

### Changed

- Folders can now be collapsed in the recipe list ([#155](https://github.com/LucasPickering/slumber/issues/155))
- Improvements to Insomnia import ([#12](https://github.com/LucasPickering/slumber/issues/12))
- Rename `import-experimental` command to `import`
  - It's official now! It's still going to get continued improvement though
- Show `WARN`/`ERROR` log output for CLI commands
- Validate recipe `method` field during deserialization instead of on request init
  - This means you'll get an error on startup if your method is invalid, instead of when you go to run the request
  - This is not a breaking change because if you had an incorrect HTTP method, the request still didn't _work_ before, it just broke later
- Arguments to chains are now treated as templates ([#151](https://github.com/LucasPickering/slumber/issues/151))
  - Support fields are `path` for `!file` chains, `command` for `!command` chains, and `message` for `!prompt` chains
  - This means you can now _really_ chain chains together!

## [0.18.0] - 2024-04-18

### Breaking

- All existing recipes must be tagged with `!request` in the collection file
  - This is necessary to differentiate from the new `!folder` type
- Profile values are always treated as templates now
  - Any profile values that were previously the "raw" variant (the default) that contain template syntax (e.g. `{{user_id}}`) will now be rendered as templates. In reality this is very unlikely, so this probably isn't going to break your setup
  - If you have an existing profile value tagged with `!template` it **won't** break, but it will no longer do anything
- Unknown fields in config/collection files will now be rejected ([#154](https://github.com/LucasPickering/slumber/issues/154))
  - In most cases this field is a mistake, so this is meant to make debugging easier
  - If you have an intentional unknown field, you can now nest it under `.ignore` to ignore it
- Replace `slumber show dir` with `slumber show paths`

### Added

- Request recipes can now be organized into folders ([#60](https://github.com/LucasPickering/slumber/issues/60))
  - See [the docs](https://slumber.lucaspickering.me/book/api/request_collection/request_recipe.html#folder-fields) for usage examples
- Add `slumber show config` and `slumber show collection` subcommands

### Changed

- Prevent infinite recursion in templates
  - It now triggers a helpful error instead of a panic
- Support additional key codes for input mapping, including media keys

### Fixed

- Multiple spaces between modifiers/key codes in a key combination are now ignored

## [0.17.0] - 2024-04-08

### Breaking

- All variants of the `Chain.source` field are now maps
  - This is to support the next request auto-execution feature, as well as future proofing for additional chain configuration
- Remove `send_request` keybinding
  - The `submit` keybinding is now used to send requests from all panes (except the profile pane)
  - This is only a breaking change if you have `send_request` remapped in your config file

Follow this mapping to update:

```yaml
# Before
chains:
  auth_token:
    source: !request login
  username:
    source: !command ["echo", "-n", "hello"]
  username:
    source: !file ./username.txt
  password:
    source: !prompt Enter Password
---
# After
chains:
  auth_token:
    source: !request
      recipe: login
  username:
    source: !command
      command: ["echo", "-n", "hello"]
  username:
    source: !file
      path: ./username.txt
  password:
    source: !prompt
      message: Enter Password
```

### Added

- Chained requests can now be auto-executed according to various criteria ([#140](https://github.com/LucasPickering/slumber/issues/140))
  - See [the docs](https://slumber.lucaspickering.me/book/user_guide/chaining_requests.html) for more
- Add Authentication tab to recipe pane ([#144](https://github.com/LucasPickering/slumber/issues/144))

### Changed

- Don't print full stack trace for failed CLI commands
- Disable formatting and highlighting for response bodies over 1MB (size threshold customizable [in the config](https://slumber.lucaspickering.me/book/api/configuration/index.html))

### Fixes

- Improve performance of handling large response bodies

## [0.16.0] - 2024-04-01

### Added

- Add support for custom keybindings ([#137](https://github.com/LucasPickering/slumber/issues/137))

### Fixed

- Fix request body not updating in UI when changing recipe

## [0.15.0] - 2024-03-24

### Added

- Add horizontal scrolling to response body ([#111](https://github.com/LucasPickering/slumber/issues/111))
  - Use shift+left and shift+right
- Add app version to help modal
- Add "Copy as cURL" action to recipe menu ([#123](https://github.com/LucasPickering/slumber/issues/123))
- Add hotkeys to select different panes
- Add pane for rendered request
- Show response size in Response pane ([#129](https://github.com/LucasPickering/slumber/issues/129))

### Changed

- Run prompts while rendering request URL/body to be copied
- Improve UI design of profile pane
- Show raw bytes for binary responses

### Fixed

- Reset response body query when changing recipes ([#133](https://github.com/LucasPickering/slumber/issues/133))

## [0.14.0] - 2024-03-18

### Added

- Add config option `ignore_certificate_hosts` ([#109](https://github.com/LucasPickering/slumber/issues/109))
- Add menu action to open collection file in editor ([#105](https://github.com/LucasPickering/slumber/issues/105))
- Add `authentication` field to request recipe ([#110](https://github.com/LucasPickering/slumber/issues/110))

### Fixed

- Fix prompt in TUI always rendering as sensitive ([#108](https://github.com/LucasPickering/slumber/issues/108))
- Fix content type identification for extended JSON MIME types ([#103](https://github.com/LucasPickering/slumber/issues/103))
- Use named records in binary blobs in the local DB
  - This required wiping out existing binary blobs, meaning **all request history and UI state will be lost on upgrade**
- Fix basic auth in Insomnia import

## [0.13.1] - 2024-03-07

### Changed

- Move checkbox to left side of toggle tables

### Fixed

- Fix scrolling on response body pane

## [0.13.0] - 2024-02-21

### Added

- New informational flags to `slumber request`
  - `--exit-status` to set exit code based on response status ([#97](https://github.com/LucasPickering/slumber/issues/97))
  - `--status`, `--headers`, and `--no-body` to control printed output
- Filter response via JSONPath ([#78](https://github.com/LucasPickering/slumber/issues/78))

## [0.12.1] - 2024-01-22

### Changed

- Improved styling of toggled table rows

## [0.12.0] - 2024-01-07

### Added

- Move app-level configuration into a file ([#89](https://github.com/LucasPickering/slumber/issues/89))
  - Right now the only supported field is `preview_templates`
- Toggle query parameters and headers in recipe pane ([#30](https://github.com/LucasPickering/slumber/issues/30))
  - You can easily enable/disable parameters and headers without having to modify the collection file now
- Add Copy URL action, to get the full URL that a request will generate ([#93](https://github.com/LucasPickering/slumber/issues/93))

### Changed

- Show profile contents while in the profile list ([#26](https://github.com/LucasPickering/slumber/issues/26))
- Remove settings modal in favor of the settings file
  - Supporting changing configuration values during a session adds a lot of complexity

## [0.11.0] - 2023-12-20

### Added

- Add action to copy entire request/response body ([#74](https://github.com/LucasPickering/slumber/issues/45))
- Persist UI state between sessions ([#39](https://github.com/LucasPickering/slumber/issues/39))
- Text window can be controlled with PgUp/PgDown/Home/End ([#77](https://github.com/LucasPickering/slumber/issues/77))
- Add back manual reload keybinding (R)
  - Mostly for development purposes
- Add collection ID/path to help modal ([#59](https://github.com/LucasPickering/slumber/issues/59))
  - Also add collection ID to terminal title
- Add new docs for templates and collection reuse ([#67](https://github.com/LucasPickering/slumber/issues/67))

### Changed

- [BREAKING] Key profiles/chains/requests by ID in collection file
- [BREAKING] Merge request history into a single DB file
  - Request history (and UI state) will be lost
- [BREAKING] `show` subcommand now takes a `target` argument
  - Right now the only option is `slumber show dir`, which has the same behavior as the old `slumber show` (except now it prints the bare directory)
- [BREAKING] Remove option to toggle cursor capture
  - Turns out it's not that useful, since most terminals provide override behavior
- Filter request history by profile ([#74](https://github.com/LucasPickering/slumber/issues/74))
- Hide sensitive chain values in preview
- Change fullscreen keybinding from F11 to F
  - F11 in some cases is eaten by the IDE or OS, which is annoying

### Fixed

- Don't require collection file to be present for `show` subcommand ([#62](https://github.com/LucasPickering/slumber/issues/62))
- Fix state file being created in root Slumber directory if collection file is invalid ([#71](https://github.com/LucasPickering/slumber/issues/71))
- Fix pane cycling while in fullscreen ([#76](https://github.com/LucasPickering/slumber/issues/76))

## [0.9.0] - 2023-11-28

### Added

- Add setting to toggle cursor capture
- Add help modal
- Add cursor navigation

### Changed

- Always show help text in footer, regardless of notification state
- Add highlight border to fullscreened pane
- Allow exiting fullscreen mode with ESC

## [0.8.0] - 2023-11-21

### Added

- Add `slumber show` subcommand

### Changed

- Remove keybinding to reload collection
  - Not useful now that the TUI has automatic reloading
- Move to stable Rust channel and add MSRV of 1.74

### Fixed

- Don't panic if the collection file is invalid on first startup [#34](https://github.com/LucasPickering/slumber/issues/34)
  - The TUI will now show an empty screen, and watch the collection file for changes
- Fix long status code reasons getting cut off in response header [#40](https://github.com/LucasPickering/slumber/issues/40)
- Trim leading/trailing newlines from header values to prevent validation error

## [0.7.0] - 2023-11-16

### Added

- Added recursive templates for profile values, using the `!template` tag before a value

### Changed

- Parse templates up front instead of during render
- Switch to nom for template parsing
  - Parse errors should be better now

## [0.6.0] - 2023-11-11

### Added

- Add ability to preview template values. This will show the rendered value under current settings [#29](https://github.com/LucasPickering/slumber/issues/29)
  - This includes a new modal to toggle the setting on/off, via the `X` key
- Add `command` source type for chained values, which uses stdout from an executed subprocess command [#31](https://github.com/LucasPickering/slumber/issues/31)

### Changed

- HTTP method is now a plain string, not a template string. This simplifies some internal logic, and I don't think there was a compelling reason to make a template in the first place.

## [0.5.0] - 2023-11-07

### Added

- Add top-level collection `id` field
  - Needed in order to give each collection its own history file
- Disable mouse capture to allow text highlighting [#17](https://github.com/LucasPickering/slumber/issues/17)
- Add keybinding (F2) to send request from any view

### Fixed

- Differentiate history between different collections [#10](https://github.com/LucasPickering/slumber/issues/10)
- Ensure ctrl-c can't get eaten by text boxes (it guarantees exit now) [#18](https://github.com/LucasPickering/slumber/issues/18)

### Changed

- Adjust size of profile list dynamically based on number of profiles
- Use structured table display format for query parameters and headers
- Tweak list and tab styling

## [0.4.0] - 2023-11-02

### Added

- Request and response panes can now be fullscreened and scrolled [#14](https://github.com/LucasPickering/slumber/issues/14)

### Removed

- Removed `Chain.name` field in config

### Changed

- All modals now use a shared queue

### Fixed

- Initially selected recipe loads most recent response [#13](https://github.com/LucasPickering/slumber/issues/13)

## [0.3.1] - 2023-10-22

Initial distributed release!
