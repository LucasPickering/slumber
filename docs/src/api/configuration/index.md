# Configuration

Configuration provides _application_-level settings, as opposed to collection-level settings.

## Location & Creation

Configuration is stored in the Slumber root directory, under the file `config.yml`. To find the root directory, you can run:

```sh
slumber show dir
```

To quickly create and edit the file:

```sh
# Replace vim with your favorite text editor
vim $(slumber show dir)/config.yml
```

If the root directory doesn't exist yet, you can create it yourself or have Slumber create it by simply starting the TUI.

## Fields

| Field                      | Type                                | Description                                                                                       | Default                    |
| -------------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------- | -------------------------- |
| `debug`                    | `boolean`                           | Enable developer information                                                                      | `false`                    |
| `editor`                   | `string`                            | Command to use when opening files for in-app editing. [More info](./editor.md)                    | `VISUAL`/`EDITOR` env vars |
| `ignore_certificate_hosts` | `string[]`                          | Hostnames whose TLS certificate errors will be ignored. [More info](../../troubleshooting/tls.md) | `[]`                       |
| `input_bindings`           | `mapping[Action, KeyCombination[]]` | Override default input bindings. [More info](./input_bindings.md)                                 | `{}`                       |
| `preview_templates`        | `boolean`                           | Render template values in the TUI? If false, the raw template will be shown.                      | `true`                     |
| `theme`                    | [`Theme`](./theme.md)               | Visual customizations                                                                             | `{}`                       |
