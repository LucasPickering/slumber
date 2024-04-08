# Profile

A profile is a collection of static template values. It's useful for configuring and switching between multiple different environments/settings/etc. Profile values are all templates themselves, so nested values can be used.

## Fields

| Field  | Type                                         | Description                       | Default                |
| ------ | -------------------------------------------- | --------------------------------- | ---------------------- |
| `name` | `string`                                     | Descriptive name to use in the UI | Value of key in parent |
| `data` | [`mapping[string, Template]`](./template.md) | Fields, mapped to their values    | `{}`                   |

## Examples

```yaml
local:
  name: Local
  data:
    host: localhost:5000
    url: "https://{{host}}"
    user_guid: abc123
```
