# Configuration

Configuration provides _global_ settings for all of Slumber, as opposed to collection-level settings.

## Location & Creation

By default, configuration is stored in a platform-specific configuration directory, according to [dirs::config_dir](https://docs.rs/dirs/latest/dirs/fn.config_dir.html).

| Platform | Path                                                   |
| -------- | ------------------------------------------------------ |
| Linux    | `$HOME/.config/slumber/config.yml`                     |
| MacOS    | `$HOME/Library/Application Support/slumber/config.yml` |
| Windows  | `C:\Users\<User>\AppData\Roaming\slumber\config.yml`   |

You can also find the config path by running:

```sh
slumber show paths config
```

You can open the config file in [your preferred editor](../../user_guide/tui/editor.md#editing) with:

```sh
slumber show config --edit
```

If the config directory doesn't exist yet, Slumber will create it automatically when starting the TUI for the first time.

> Note: Prior to version 2.1.0, Slumber stored configuration in a different location on Linux (`~/.local/share/slumber/config.yml`). If that file exists on your system, **it will be used in place of the newer location.** For more context, see [issue #371](https://github.com/LucasPickering/slumber/issues/371).

You can change the location of the config file by setting the environment variable `SLUMBER_CONFIG_PATH`. For example:

```sh
SLUMBER_CONFIG_PATH=~/dotfiles/slumber.yml slumber
```

## Hidden Fields

Any unknown field in the config file will be rejected, unless it is a **top-level** field beginning with `.`. You can combine this with [YAML anchors](https://yaml.org/spec/1.2.2/#anchors-and-aliases) to define reusable components in your config file.

## Fields

The following fields are available in `config.yml`:

### `commands.shell`

**Type:** `string[]`

**Default:** `[sh, -c]` (Unix), `[cmd, /S, /C]` (Windows)

Shell used to execute commands within the TUI. Use `[]` for no shell (commands will be parsed and executed directly). [More info](../../user_guide/tui/filter_query.md)

### `commands.default_query`

**Type:** `string` or `mapping[Mime, string]` (see [MIME Maps](./mime.md))

**Default:** `""`

Default query command for all responses. [More info](../../user_guide/tui/filter_query.md)

### `editor`

**Type:** `string`

**Default:** `VISUAL`/`EDITOR` env vars, or `vim`

Command to use when opening files for in-app editing. [More info](../../user_guide/tui/editor.md#editing)

### `follow_redirects`

**Type:** `boolean`

**Default:** `true`

Enable/disable following redirects (3xx status codes) automatically. If enabled, the HTTP client follow redirects [up to 10 times](https://docs.rs/reqwest/0.12.15/reqwest/index.html#redirect-policies).

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

### `persist`

**Type:** `boolean`

**Default:** `true`

Enable/disable the storage of requests and responses in Slumber's local database. This is only used in the TUI. CLI requests are _not_ persisted unless the `--persist` flag is passed, in which case they will always be persisted. [See here for more](../../user_guide/database.md).

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

**Type:** `string` or `mapping[Mime, string]` (see [MIME Maps](./mime.md))

**Default:** `less` (Unix), `more` (Windows)

Command to use when opening files for viewing. [More info](../../user_guide/tui/editor.md#paging)
