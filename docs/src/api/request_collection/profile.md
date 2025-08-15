# Profile

A profile is a collection of static template values. It's useful for configuring and switching between multiple different environments/settings/etc. Profile values are all templates themselves, so nested values can be used.

## Fields

| Field     | Type                                                               | Description                                                 | Default                |
| --------- | ------------------------------------------------------------------ | ----------------------------------------------------------- | ---------------------- |
| `name`    | `string`                                                           | Descriptive name to use in the UI                           | Value of key in parent |
| `default` | `boolean`                                                          | Use this profile in the CLI when `--profile` isn't provided | `false`                |
| `data`    | [`mapping[string, Template]`](../../user_guide/templates/index.md) | Fields, mapped to their values                              | `{}`                   |

## Examples

```yaml
profiles:
  local:
    name: Local
    data:
      host: localhost:5000
      url: "https://{{ host }}"
      user_guid: abc123
```
