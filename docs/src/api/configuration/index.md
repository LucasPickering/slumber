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

| Field                      | Type                                | Description                                                                                       | Default                    |
| -------------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------- | -------------------------- |
| `debug`                    | `boolean`                           | Enable developer information                                                                      | `false`                    |
| `editor`                   | `string`                            | Command to use when opening files for in-app editing. [More info](./editor.md)                    | `VISUAL`/`EDITOR` env vars |
| `ignore_certificate_hosts` | `string[]`                          | Hostnames whose TLS certificate errors will be ignored. [More info](../../troubleshooting/tls.md) | `[]`                       |
| `input_bindings`           | `mapping[Action, KeyCombination[]]` | Override default input bindings. [More info](./input_bindings.md)                                 | `{}`                       |
| `preview_templates`        | `boolean`                           | Render template values in the TUI? If false, the raw template will be shown.                      | `true`                     |
| `theme`                    | [`Theme`](./theme.md)               | Visual customizations                                                                             | `{}`                       |
