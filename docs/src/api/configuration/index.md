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

| Field                      | Type                                | Description                                                                                       | Default                                      |
| -------------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| `commands.shell`           | `string[]`                          | Shell used to execute commands within the TUI. [More info](#commands)                             | `[sh, -c]` (Unix), `[cmd, /S, /C]` (Windows) |
| `debug`                    | `boolean`                           | Enable developer information                                                                      | `false`                                      |
| `editor`                   | `string`                            | Command to use when opening files for in-app editing. [More info](./editor.md)                    | `VISUAL`/`EDITOR` env vars, or `vim`         |
| `ignore_certificate_hosts` | `string[]`                          | Hostnames whose TLS certificate errors will be ignored. [More info](../../troubleshooting/tls.md) | `[]`                                         |
| `input_bindings`           | `mapping[Action, KeyCombination[]]` | Override default input bindings. [More info](./input_bindings.md)                                 | `{}`                                         |
| `large_body_size`          | `number`                            | Size over which request/response bodies are not formatted/highlighted, for performance (bytes)    | `1000000` (1 MB)                             |
| `preview_templates`        | `boolean`                           | Render template values in the TUI? If false, the raw template will be shown.                      | `true`                                       |
| `theme`                    | [`Theme`](./theme.md)               | Visual customizations                                                                             | `{}`                                         |
| `pager`                    | `string`                            | Command to use when opening files for viewing. [More info](./editor.md)                           | `less` (Unix), `more` (Windows)              |
| `viewer`                   | See `pager`                         | Alias for `pager`, for backward compatibility                                                     | See `pager`                                  |

## Commands

Slumber allows you to execute shell commands within the TUI, e.g. for querying and transforming response bodies. By default, the command you enter is passed to `sh` (or `cmd` on Windows) for parsing and execution. This allows you to access shell behavior such as piping. The command to execute is passed as the final argument to the shell, and the response body is passed as stdin to the spawned process.

If you want to use a different shell (e.g. to access your shell aliases), you can override the `commands.shell` config field. For example, to use [fish](https://fishshell.com/):

```yaml
commands:
  shell: ["fish", "-c"]
```

If you don't want to use a shell at all, you can pass `[]`:

```yaml
commands:
  shell: []
```

In this case, any commands to be executed will be parsed with [shell-words](https://docs.rs/shell-words/1.1.0/shell_words/fn.split.html) and executed directly. For example, `echo -n test` will run `echo` with the arguments `-n` and `test`.
