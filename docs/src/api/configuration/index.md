# Configuration

Configuration provides _application_-level settings, as opposed to collection-level settings.

## Location & Creation

By default, configuration is stored in a platform-specific configuration directory, according to [dirs::config_dir](https://docs.rs/dirs/latest/dirs/fn.config_dir.html).

| Platform | Path                                                   |
| -------- | ------------------------------------------------------ |
| Linux    | `$HOME/.config/slumber/config.yml`                     |
| MacOS    | `$HOME/Library/Application Support/slumber/config.yml` |
| Windows  | `C:\Users\<User>\AppData\Roaming\slumber\config.yml`   |

You can also find the config path by running:

```sh
slumber show paths
```

If the config directory doesn't exist yet, Slumber will create it automatically when starting the TUI for the first time.

> Note: Prior to version 2.1.0, Slumber stored configuration in a different location on Linux (`~/.local/share/slumber/config.yml`). If that file exists on your system, **it will be used in place of the newer location.** For more context, see [issue #371](https://github.com/LucasPickering/slumber/issues/371).

You can change the location of the config file by setting the environment variable `SLUMBER_CONFIG_PATH`. For example:

```sh
SLUMBER_CONFIG_PATH=~/dotfiles/slumber.yml slumber
```

## Fields

The following fields are available in `config.yml`:

<!-- toc -->

### `commands.shell`

**Type:** `string[]`

**Default:** `[sh, -c]` (Unix), `[cmd, /S, /C]` (Windows)

Shell used to execute commands within the TUI. [More info](#commands)

### `commands.query_default`

**Type:** `string`

**Default:** `""`

Default query command for all responses. [More info](#commands)

### `debug`

**Type:** `boolean`

**Default:** `false`

Enable developer information in the TUI

### `editor`

**Type:** `string`

**Default:** `VISUAL`/`EDITOR` env vars, or `vim`

Command to use when opening files for in-app editing. [More info](../../user_guide/tui/editor.md)

### `ignore_certificate_hosts`

**Type:** `string`

**Default:** `[]`

Hostnames whose TLS certificate errors will be ignored. [More info](../../troubleshooting/tls.md)

### `input_bindings`

**Type:** `mapping[Action, KeyCombination[]]`

**Default:** `{}`

Override default input bindings. [More info](./input_bindings.md)

### `large_body_size`

**Type:** `number`

**Default:** `1000000` (1 MB)

Size over which request/response bodies are not formatted/highlighted, for performance (bytes)

### `preview_templates`

**Type:** `boolean`

**Default:** `true`

Render template values in the TUI? If false, the raw template will be shown.

### `theme`

**Type:** `Theme`

**Default:** `{}`

Visual customizations for the TUI. [More info](./theme.md)

### `pager`

**Alias:** `viewer` (for historical compatibility)

**Type:** `string`

**Default:** `less` (Unix), `more` (Windows)

Command to use when opening files for viewing. [More info](../../user_guide/tui/editor.md)
