# Changelog

## [Unreleased] - ReleaseDate

### Changed

- Folders can now be collapsed in the recipe list ([#155](https://github.com/LucasPickering/slumber/issues/155))
- Improvements to Insomnia import ([#12](https://github.com/LucasPickering/slumber/issues/12))
- Rename `import-experimental` command to `import`
  - It's official now! It's still going to get continuted improvement though
- Show `WARN`/`ERROR` log output for CLI commands
- Validate recipe `method` field during deserialization instead of on request init
  - This means you'll get an error on startup if your method is invalid, instead of when you go to run the request
  - This is not a breaking change because if you had an incorrect HTTP method, the request still didn't _work_ before, it just broke later

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
