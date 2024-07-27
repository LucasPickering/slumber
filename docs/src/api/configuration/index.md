# Configuration

Configuration provides _application_-level settings, as opposed to collection-level settings. Configuration **only applies to the TUI**. The CLI is not impacted by the configuration file. Config fields that are relevant to the CLI are also available as CLI flags.

## Location & Creation

Configuration is stored in the Slumber root directory, under the file `config.yml`. To find the root directory, you can run:

```sh
slumber show paths
```

You can also find the config path by running the TUI and opening the help menu with `?`. If the root directory doesn't exist yet, you can create it yourself or have Slumber create it by simply starting the TUI.

## Fields

| Field                      | Type                                | Description                                                                                       | Default |
| -------------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------- | ------- |
| `preview_templates`        | `boolean`                           | Render template values in the TUI? If false, the raw template will be shown.                      | `true`  |
| `ignore_certificate_hosts` | `string[]`                          | Hostnames whose TLS certificate errors will be ignored. [More info](../../troubleshooting/tls.md) | `[]`    |
| `input_bindings`           | `mapping[Action, KeyCombination[]]` | Override default input bindings. [More info](./input_bindings.md)                                 | `{}`    |
| `theme`                    | [`Theme`](./theme.md)               | Visual customizations                                                                             | `{}`    |
