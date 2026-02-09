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
slumber config --path
```

You can open the config file in [your preferred editor](../../user_guide/tui/editor.md#editing) with:

```sh
slumber config --edit
```

If the config directory doesn't exist yet, Slumber will create it automatically when starting the TUI for the first time.

> Note: Prior to version 2.1.0, Slumber stored configuration in a different location on Linux (`~/.local/share/slumber/config.yml`). If that file exists on your system, **it will be used in place of the newer location.** For more context, see [issue #371](https://github.com/LucasPickering/slumber/issues/371).

You can change the location of the config file by setting the environment variable `SLUMBER_CONFIG_PATH`. For example:

```sh
SLUMBER_CONFIG_PATH=~/dotfiles/slumber.yml slumber
```

## Hidden Fields

Any unknown field in the config file will be rejected, unless it is a **top-level** field beginning with `.`. You can combine this with [YAML anchors](https://yaml.org/spec/1.2.2/#anchors-and-aliases) to define reusable components in your config file.

```yaml
.hidden_field:
  my_color: red

theme:
  primary_color:
    $ref: "#/.hidden_field/my_color"
```

## Fields

The following fields are available in `config.yml`:

### `commands.shell`

**Type:** `string[]`

**Default:** `[sh, -c]` (Unix), `[cmd, /S, /C]` (Windows)

Shell used to execute commands within the TUI. Use `[]` for no shell (commands will be parsed and executed directly).

[See Data Filtering & Querying for more info](../../user_guide/tui/filter_query.md).

#### Example

```yaml
commands:
  shell: ["fish", "--no-config", "-c"]
```

### `commands.default_query`

**Type:** `string` or `mapping[Mime, string]` (see [MIME Maps](./mime.md))

**Default:** `""`

Default query command for all responses.

[See Data Filtering & Querying for more info](../../user_guide/tui/filter_query.md).

#### Example

```yaml
commands:
  default_query:
    json: jq
```

### `editor`

**Type:** `string`

**Default:** `VISUAL`/`EDITOR` env vars, or `vim`

Command to use when opening files for in-app editing.

[See In-App Editing for more info](../../user_guide/tui/editor.md#editing).

#### Example

```yaml
editor: "hx"
```

### `follow_redirects`

**Type:** `boolean`

**Default:** `true`

Enable/disable following redirects (3xx status codes) automatically. If enabled, the HTTP client follow redirects [up to 10 times](https://docs.rs/reqwest/0.12.15/reqwest/index.html#redirect-policies).

#### Example

```yaml
follow_redirects: false
```

### `ignore_certificate_hosts`

**Type:** `string`

**Default:** `[]`

Hostnames whose TLS certificate errors will be ignored. These values are _not_ wildcards; certificates will only be ignored for **exact matches**.

[See TLS Certificate Errors for more info](../../troubleshooting/tls.md).

#### Example

```yaml
ignore_certificate_hosts: ["my-site.local"]
```

In this case, any requests to `https://my-site.local/` will _not_ receive TLS certificate validation.

### `input_bindings`

**Type:** `mapping[Action, KeyCombination[]]`

**Default:** `{}`

Override default input bindings.

[See Input Bindings for more info](./input_bindings.md).

#### Example

```yaml
input_bindings:
  up: [k]
  down: [j]
  left: [h]
  right: [l]
  scroll_left: [shift h]
  scroll_right: [shift l]
```

### `large_body_size`

**Type:** `number`

**Default:** `1000000` (1 MB)

Size over which request/response bodies are not formatted/highlighted, for performance (bytes)

#### Example

```yaml
large_body_size: 100000 # 100KB
```

### `persist`

**Type:** `boolean`

**Default:** `true`

Enable/disable the storage of requests and responses in Slumber's local database. This is only used in the TUI. CLI requests are _not_ persisted unless the `--persist` flag is passed, in which case they will always be persisted.

[See Database & Persistence for more info](../../user_guide/database.md).

#### Example

```yaml
persist: false # Requests/responses will deleted upon closing a session
```

### `pager`

**Alias:** `viewer` (for historical compatibility)

**Type:** `string` or `mapping[Mime, string]` (see [MIME Maps](./mime.md))

**Default:** `less` (Unix), `more` (Windows)

Command to use when opening files for viewing.

[See In-App Paging for more info](../../user_guide/tui/editor.md#paging).

#### Example

```yaml
pager:
  json: fx
  "*/*": bat
```

### `preview_templates`

**Type:** `boolean`

**Default:** `true`

Render template values in the TUI? If false, the raw template will be shown.

#### Example

```yaml
preview_templates: false
```

### `theme`

**Type:** `Theme`

**Default:** `{}`

Visual customizations for the TUI.

[See Theme for more info](./theme.md).

#### Example

```yaml
theme:
  primary_color: red
```
