# Changelog

All user-facing changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - ReleaseDate

<!-- ANCHOR: changelog -->

5.0 is a huge release that focuses on two main areas:

- A major refactor of the TUI includes:
  - A new layout with a collapsible sidebar to speed up navigation
  - Query/export command history navigation (similar to shell history)
  - QoL improvements such as selecting list items by click
- CLI commands have been reorganized to be more consistent and discoverable

While is is a major release with breaking changes, the breakages are fairly limited. There are no changes to the collection format, so it's unlikely you'll need to make any changes to your workflow to upgrade.

### Breaking

The breaking changes in this release are mostly limited to CLI commands. The only change to the collection format is that JSON keys are now treated as templates. This will only impact you if you have any JSON **keys** containing `{{`. If you want these to be treated literally, you'll need to [escape them](https://slumber.lucaspickering.me/user_guide/templates/index.html#escape-sequences): `{_{`

- JSON body object keys are now parsed as templates and can be dynamically modified (@SignalWhisperer [#698](https://github.com/LucasPickering/slumber/issues/698))
- `slumber show` command has been removed; its functionality has be split up across a set of more discoverable subcommands:
  - `slumber show config` -> `slumber config`
  - `slumber show collection` -> `slumber collection`
  - `slumber show paths` removed
  - `slumber show paths config` -> `slumber config --path`
  - `slumber show paths collection` -> `slumber collection --path`
  - `slumber show paths db` -> `slumber db --path`
  - `slumber show paths log` -> `slumber --print-log-path`
- `slumber collections` and `slumber history` have been moved under `slumber db`
  - `slumber collections ...` -> `slumber db collection ...`
  - `slumber history ...` -> `slumber db request ...`
  - The goal is to group all commands related to direct DB access together. These are advanced/niche commands that are often used together.
- Logs are now written to temporary files. Each Slumber session uses a different log file. To find the log file:
  - In the TUI, open the help menu with `?`
  - CLI commands that fail will automatically print the log path. You can also pass `--print-log-path` to have it always print
- Remove `left_click` and `right_click` mappable actions
  - Mouse clicks can no longer be mapped to keys
- Replace the `RUST_LOG` environment variable with a `--log-level` argument
  - ERROR/WARN log output is not longer shown by default in stderr for CLI commands

### Added

- Add `Copy as CLI` action to generate a `slumber request` CLI command for the selected recipe/profile
- Add `Copy as Python` action to generate Python code that uses the [slumber-python](https://pypi.org/project/slumber-python/) API to make a request with the selected recipe/profile
- Add flags to override various parts of a recipe from the CLI
  - `--url <url>` overrides the entire URL
  - `--query <param=value>` overrides a single query parameter
  - `--header <header=value>` (`-H`) overrides a header
  - `--basic <username:password>` sets basic authentication
  - `--bearer <token>` sets bearer authentication
  - `--body <body>` sets the request body
  - `--form <field=value>` (`-F`) overrides a form field
- Add `slumber config` command (replaces `slumber show config`)
- Add `slumber collection` command (replaces `slumber show collection`)
- Add `slumber db --path` flag (replaces `slumber show paths db`)
- You can now search command history in the query/export command text box
  - Up/down to cycle through past commands
  - Ctrl-r to search
  - Command history is specific to each collection and capped at 100 commands per collection

### Changed

- Reconfigured the TUI layout. The main change is that the recipe, profile, and history lists open in an expandable sidebar now
  - Switching between recipes and profiles should feel faster and more intuitive now
- `prompt()` and `select()` calls are now grouped into a single form pane in the TUI
  - Previously, you'd be fed a series of modals one-by-one to fill out
- Some menu actions have been moved into nested sections for better organization
- Refactor significant portions of the TUI logic. There should be no user-facing changes, but if you notice any bugs [please report them!](https://github.com/LucasPickering/slumber/issues/new/choose)
- List items can now be selected by click
- Recipe templates are now edited inline instead of in a pop-up modal
- Help modal has been moved to a fullscreen page
- Make `slumber request` aliases `rq` and `req` visible
- `slumber db collection delete` now accepts more than 1 collection at a time

### Fixed

- Invalid body override template is displayed instead of being thrown away [#531](https://github.com/LucasPickering/slumber/issues/531)
- Fix panic when SIGTERM is sent to a TUI process that failed to start and is display a collection error
- Fix indentation in TUI display of multi-line errors
- Fix collection file watching for vim, helix, and other editors that swap instead of writing [#706](https://github.com/LucasPickering/slumber/issues/706)
  - Previously, the file watching would break after the first write because these editors replace the edited file (specifically, the inode) instead of just writing to it

## [4.3.1] - 2026-01-02

<!-- ANCHOR: changelog -->

### Changed

- Add line breaks to cURL command output if length is >100 characters (@fgebhart, [#678](https://github.com/LucasPickering/slumber/issues/678))

## [4.3.0] - 2025-12-12

<!-- ANCHOR: changelog -->

### Added

- Add `default` keyword arg to `env()` [#652](https://github.com/LucasPickering/slumber/issues/652)
- Add string maniuplation functions [#655](https://github.com/LucasPickering/slumber/issues/655)
  - `split()` splits a string on a separator
  - `join()` joins an array on a separator
  - `index()` gets a single element from a string or array
  - `slice()` slices a portion out of a string or array
  - `upper()` and `lower()` convert strings to upper/lower case respectively
  - `replace()` replaces occurrences of one string (or regex) with another

### Changed

- Default config file no longer contains all known configuration fields
  - Instead it's just a single example field and a link to the docs now
  - If you have an old default file, it will be replaced by the new one. Any file that's been modified from the default in any way (including just whitespace/comments) will **not** be modified
  - See [#670](https://github.com/LucasPickering/slumber/pull/670) for more

## [4.2.1] - 2025-11-26

<!-- ANCHOR: changelog -->

### Fixed

- Fix crash when previewing a JSON body that contains an escaped quote [#646](https://github.com/LucasPickering/slumber/issues/646)

## [4.2.0] - 2025-10-14

<!-- ANCHOR: changelog -->

### Added

- Add [Python bindings](https://slumber.lucaspickering.me/integration/python.html), allowing you to use your Slumber collection from Python scripts without having to invoke the CLI
- Add [`jq`](https://slumber.lucaspickering.me/api/template_functions.html#jq) function to the template language, to query and transform JSON with [jq](https://jqlang.org/manual/)
- `select()` now accepts a list of objects `{"label": "Label", "value": "Value"}` in addition to a list of strings [#609](https://github.com/LucasPickering/slumber/issues/609)
  - This allows you to pass a list of values where the returned value is different from the string you see in the select list.
  - For example, to select a user from a list where you select users by name but the returned value is their ID: `select([{"label": "User 1", "value": 1}, {"label": "User 2", "value": 2}])`
  - [See docs for more](https://slumber.lucaspickering.me/api/template_functions.html#select)
- Add URL tab to the recipe pane, allowing the URL to be temporarily overridden

### Changed

- `Edit Collection` TUI has been replaced by `Edit Recipe`, which opens the file to the selected recipe
  - This will make it much easier to make tweaks to a recipe
- The `Body` and `Authentication` tabs of the `Recipe` pane are now disabled if the recipe doesn't have a body/authentication (respectively)
- Disabled actions can no longer be selected in the action menu
- `slumber show config --edit` and `slumber show collection --edit` now display the error if the file is invalid after editing

## [4.1.0] - 2025-09-30

<!-- ANCHOR: changelog -->

### Added

- Add support for streaming large HTTP bodies with the new `stream` body type [#256](https://github.com/LucasPickering/slumber/issues/256)
  - [See docs](https://slumber.lucaspickering.me/user_guide/streaming.html)

### Changed

- `form_multipart` fields that consist of a single call to `file()` will now include the correct `Content-Type` header and `filename` field of the `Content-Disposition` header [#582](https://github.com/LucasPickering/slumber/issues/582)
  - [See docs](https://slumber.lucaspickering.me/user_guide/streaming.html)
- Previews of `prompt()` will now show the default value if possible
- Recipe ID is now shown in the top-right of the Recipe pane header

### Fixed

- Fix `slumber generate curl` output for multipart forms with file fields
- `slumber import insomnia` now imports some dynamic expressions. Values chained from other responses now properly import as `response()`/`response_header()` calls [#164](https://github.com/LucasPickering/slumber/issues/164)
- Improve TUI performance when handling lots of input events

## [4.0.1] - 2025-09-14

<!-- ANCHOR: changelog -->

### Fixed

- Remove `output` argument for `command()`
  - This wasn't intended to be released and didn't actually work

## [4.0.0] - 2025-09-12

[Migration guide](https://slumber.lucaspickering.me/other/v4_migration.html)

### Highlights

4.0 is Slumber's largest release to date, with a number of exciting improvements to the collection format. The overall goal of this release is to make collection files:

- Easier to read
- Easier to write
- Easier to share

This required a number of breaking changes. For upgrade instructions, see the `Breaking` section.

#### Goodbye chains, we won't miss you

Previously, templates could source dynamic data (such as data from other responses, files, commands, etc.) via _chains_. While powerful, they were annoying to use because you had to define your chain in one part of the collection file, then use it in another. This led to a lot of jumping around, which was especially annoying for a simple chain that only got used once. Additionally, chains were clunky and unintuitive to compose together. You could combine multiple chains together (hence the name), but it wasn't obvious how.

4.0 eliminates chains entirely, replacing them with functions directly in your templates, inspired by [Jinja](https://jinja.palletsprojects.com/en/stable/) (but dramatically simplified). Here's a side-by-side comparison:

**Before**

```yaml
chains:
  fish_ids:
    source: !request
      recipe: list_fish
      trigger: !expire 1d
    selector: $[0].id
    selector_mode: array
  fish_id:
    source: !select
      options: "{{fish_ids}}"

requests:
  list_fish:
    method: GET
    url: "{{host}}/fishes"
  get_fish:
    method: GET
    url: "{{host}}/fishes/{{fish_id}}"
```

**After**

```yaml
requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
  get_fish:
    method: GET
    url: "{{ host }}/fishes/{{ response('fish_list', trigger='1d') | jsonpath('$[*].id', mode='array') | select() }}"
```

So much easier to follow!

[See docs for more](https://slumber.lucaspickering.me/user_guide/templates/functions.html).

#### Share configuration between collection files with `$ref`

YAML merge syntax (`<<: *alias`) is no longer supported. Instead, the more flexible JSON reference (`$ref`) format is supported. This allows you to reuse any portion of the current collection _without having to declare it as an alias_. Even better though, **you can import components from other files:**

```yaml
# slumber.yml
requests:
  login:
    $ref: "./common.yml#/requests/login"
```

[See docs for more](https://slumber.lucaspickering.me/user_guide/composition.html).

#### JSON Schema

Slumber now exports a [JSON Schema](https://jsonschema.com) for both its global config and request collection formats. This makes it possible to get validation and completion in your IDE. To make this possible we've ditched the YAML `!tag` syntax in favor of `type` fields within each block.

[See docs for more](https://slumber.lucaspickering.me/user_guide/json_schema.md).

[Thanks to @anussell5559 for this suggestion](https://github.com/LucasPickering/slumber/issues/374).

### Breaking

This release contains a number of breaking changes to the collection format. The major one is a change in the template format, but there are a few other quality of life improvements as well.

You can automatically migrate your collection to the new v4 format with:

```sh
slumber import v3 <old file> <new file>
```

The new collection _should_ be equivalent to the old one, but you should keep your old version around just in case something broke. If you notice any differences, please [file a bug!](https://github.com/LucasPickering/slumber/issues/new).

[**See the migration guide for more details**](https://slumber.lucaspickering.me/other/v4_migration.html)

- Replace template chains with a more intuitive function syntax
  - Instead of defining chains separately then referencing them in templates, you can now call functions directly in templates: `{{ response('login') | jsonpath('$.token') }}`
  - [See docs for more](https://slumber.lucaspickering.me/user_guide/templates/functions.html)
- Remove YAML `!tags` in favor of an inner `type` field
  - This change makes the format compatible with JSON Schema
  - Impacts these collection nodes:
    - Authentication
    - Body
    - Folder/request nodes (`type` field not required at all; node type is inferred from the object structure)
- Represent query parameters as a map of `{parameter: value}` instead of a list of strings like `parameter=value`
  - The map format has been supported as well, but did not allow for multiple values for the same value, hence the need for the string format
  - To define multiple values for the same value, you can now use a list associated to the parameter: `{parameter: [value1, value2]}`
  - [See docs](https://slumber.lucaspickering.me/api/request_collection/query_parameters.html) for examples of the new format
- YAML anchor/alias/merge syntax has been replaced with `$ref` references, similar to OpenAPI [#290](https://github.com/LucasPickering/slumber/issues/290)
  - These references are much more flexible, including the ability to import from other files
  - [See docs](https://slumber.lucaspickering.me/user_guide/composition.html) for examples
- Commands in templates (previously `!command`, now `command()`) now fail if the command exits with a non-zero status code
- Templates in a JSON body with a single dynamic chunk (such as `{{ username }}`) will now be unpacked into their inner value rather than always being stringified
  - This means you can now create dynamic non-string values within a JSON body
  - [See docs](https://slumber.lucaspickering.me/user_guide/recipes.html#body) for more

## Added

- Generate JSON Schema for both the collection and config formats [#374](https://github.com/LucasPickering/slumber/issues/374)
  - This enables better validation and completion in your IDE; [see docs for more](https://slumber.lucaspickering.me/user_guide/json_schema.md)

<!-- ANCHOR: changelog -->

## [3.4.0] - 2025-08-17

This release focuses on improving collection history management. The `slumber collections` subcommand is fairly niche, but is now a bit more powerful and user friendly.

### Added

- Add `slumber collections delete` subcommand to remove collections from history
- Add `slumber history rm` as an alias for `slumber history delete`
- Add filter box to collection select dialog. Less scrolling, more typing!

### Changed

- `slumber collections migrate` now accepts collection IDs in addition to paths
- `slumber collections list` now includes an ID column in its output

### Fixed

- Shell completion for the global `--file`/`-f` flag will now only `.yml`/`.yaml` files
- `slumber collections migrate` now accepts paths for files that don't exist on disk (as long as they existed as collections at some point)

<!-- ANCHOR: changelog -->

## [3.3.0] - 2025-07-23

### Added

- Add collection switcher modal [#536](https://github.com/LucasPickering/slumber/issues/536)
  - You can now switch between any collection that you've opened in the past without having to close Slumber
- Add optional root-level `name` field to collections
  - This allows you to provide a descriptive name for the collection to show in the new switcher modal
  - The name is purely descriptive and does not need to be unique

### Changed

- Update to Rust 1.88

### Fixed

- Fix empty actions modal queuing when opening actions while another modal is already open
- Fix crash when a select modal is opened with a very long option

## [3.2.0] - 2025-06-20

### Added

- Add config field `follow_redirects` to enable/disable following 3xx redirects (enabled by default)
  - The behavior has always been to follow redirects, so this adds the ability to disable that globally
  - **Reminder:** Global configuration is not automatically reloaded. After making changes to your `config.yml`, you'll need to restart Slumber for changes to take effect
  - [See docs for more](https://slumber.lucaspickering.me/book/api/configuration/index.html#follow_redirects)
- Add optional `target` argument to `slumber show paths` to show just a single path
  - E.g. `slumber show paths config` prints just the config path
- Add `--edit` flag to `slumber show config` and `slumber show collection`
  - This will open the global config/collection file in your configured editor, similar to `git config --edit`
- `slumber import` now supports importing from stdin or a URL
  - If no input argument is given, it will read from stdin, e.g. `slumber import openapi < openapi.json`
  - If a URL is given, the file will be downloaded and imported, e.g. `slumber import openapi https://example.com/openapi.json`
- Add OpenAPI v3.1 importer [#513](https://github.com/LucasPickering/slumber/issues/513)

### Changed

- Any top-level fields in the config or collection file beginning with `.` will now be ignored
  - The goal is to support "hidden" fields to store reusable components. YAML aliases can be used to pull those components into various parts of your collection
  - Previously the field `.ignore` was specially supported in the collection format for this purpose; this is a generalization of that special case.

## Fixed

- Import JSON bodies from OpenAPI spec operations that don't have an `example` field
  - Now it will infer a body from the schema definition if no examples are provided
- Fix occasional hang when opening `fx` as a pager [#506](https://github.com/LucasPickering/slumber/issues/506)

## [3.1.3] - 2025-06-07

### Fixed

- Fix pager view action (regression in 3.1.2)
- Fully restore terminal before exiting to editor/pager
  - This means any stdout/stderr that the external process writes will be properly formatted in the terminal

## [3.1.2] - 2025-05-30

### Changed

- Use a dedicated error state if collection fails to load on TUI launch
- Update dependency `persisted` to 1.0
  - A few pieces of your UI state, such as selected tabs, will be lost during the upgrade due to this

### Fixed

- Fix TUI crash when using an empty select list
- Fix automatic collection reloading in Windows
  - I don't use Windows so I'm not sure exactly what scenarios it may have been broken in, but new unit tests indicate it's working now
- Fix config loading failing for read-only config files ([#504](https://github.com/LucasPickering/slumber/issues/504))

## [3.1.1] - 2025-04-23

### Fixed

- Persist response query commands separately for each content type
  - This prevents commands from running on the incorrect content type when the response type changes

## [3.1.0] - 2025-04-04

This releases focuses on history and data management. A suite of new features and improvements make it easy to disable request persistence and delete past requests from history.

### Added

- Add `--persist` flag to `slumber request`
  - By default, CLI-based requests are not stored in the history database. Use this flag to enable persistence for the sent request.
- Add `slumber history delete` subcommand for deleting request history
- Add `slumber db` subcommand to open a shell to the local SQLite database
- Add `persist` field to the global config and individual recipes
  - Both default to `true`, but you can now set them to `false` to disable data persistence a single recipe, or all instances of the app. [See here for more](https://slumber.lucaspickering.me/book/user_guide/database.html#controlling-persistence)
- Add actions to delete requests from the TUI
  - Delete a single request from the history modal or the Request/Response pane
  - Delete all requests for a recipe from the Recipe List/Recipe panes

### Changed

- Upgrade to Rust 1.86 (2024 edition!)
- Improve functionality of `slumber history list`
  - `recipe` argument is optional now. Omit it to show requests for all recipes in the current collection
  - Add `--all` argument to show requests for all collections
  - Add `--id-only` flag to print only IDs with no headers. Combine with `slumber history delete` for great success!
- Improve format of `slumber history list` table output

### Fixed

- Fix output format of `slumber request --dry-run ...` to match `slumber request --verbose`
- Fix `curl` output for URL-encoded and multipart forms
- Fix selected request not changing when profile changes

## [3.0.1] - 2025-02-19

### Fixed

- Text box now scrolls to the cursor when it's off screen
- Fix panics when the screen gets very small [#469](https://github.com/LucasPickering/slumber/issues/469)

## [3.0.0] - 2025-02-15

A major release! The main focus of this release is the introduction of shell commands for data querying and export. Previously, you could query response bodies within the TUI only using JSONPath. This limited querying only to JSON responses, and the limited amount of operators supported by JSON. Now, you can use whatever shell commands you want (such as `head`, `grep`, and `jq`) to filter your reponses bodies, right in the TUI! [Check out the docs](https://slumber.lucaspickering.me/book/user_guide/tui/filter_query.md) for more examples.

In addition to the querying change, this release includes a handful of breaking changes, none of which are likely to cause issues for existing users.

### Breaking

- Don't store CLI requests in history
- Simplify display for `slumber request`
  - The flags `--status`, `--headers` and `--no-body` have been removed in favor of a single `--verbose` flag
- Remove DB migration to upgrade from the pre-1.8.0 DB format
  - This only impacts users upgrading to 3.0.0 from versions _before_ 1.8.0. You'll need to upgrade to an intermediate version first. If you install 3.0.0 and try to start it, you'll see an error message explaining how to fix it.
  - See [#306](https://github.com/LucasPickering/slumber/issues/306) for more info

### Added

- Replace JSONPath querying with general purpose shell commands for querying response bodies. [See docs](https://slumber.lucaspickering.me/book/user_guide/tui/filter_query.md)
  - Now you can access any CLI tools you want for transforming response bodies, such as `jq` or `grep`
  - By default, commands are executed via `sh` (or `cmd` on Windows), but this is configured via the [`commands.shell` field](https://slumber.lucaspickering.me/book/api/configuration/index.html)
- Add keybind (`:` by default) to run an "export" command with a response body, allowing you to run arbitrary shell commands to save a response body to a file, copy it to the clipboard, etc. [See docs](https://slumber.lucaspickering.me/book/user_guide/tui/filter_query.md#exporting-data)
- Add `slumber history` subcommand. Currently it has two operations:
  - `slumber history list` lists all stored requests for a recipe
  - `slumber history get` prints a specific request/response
- Add `--output` flag to `slumber request` to control where the response body is written to
- Support MIME type mapping for `pager` config field, so you can set different pagers based on media type. [See docs](https://slumber.lucaspickering.me/book/api/configuration/mime.html)
- Several changes related to keybinds and action menus to make the two feel more cohesive
  - Add "Edit" and "Reset" actions to menus on the recipe pane
    - These don't provide any new functionality, as the `e` and `z` keys are already bound to those actions, but it should make them more discoverable
  - Add keybind (`v` by defualt) to open a recipe/request/response body in your pager
    - Previously this was available only through the actions menu
  - "View Body" and "Copy Body" actions for a **recipe** are now only available within the Body tab of the Recipe pane
    - Previously they were available anywhere in the Recipe List or Recipe panes. With the addition of other actions to the menu it was started to feel cluttered

### Changed

- Denote templates that have been edited during the current session with italics instead of a faint "(edited)" note
- Header names in recipes are now lowercased in the UI
  - They have always been lowercased when the request is actually sent, so now the UI is just more representative of what will be sent
- Accept a directory for the `--file`/`-f` CLI argument
  - If a directory is given, the [standard rules for detecting a collection file](https://slumber.lucaspickering.me/book/api/request_collection/index.html#format--loading) will be applied from that directory

### Fixed

- Fix certain recipe-related menu actions being enabled when they shouldn't be

## [2.5.0] - 2025-01-06

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
