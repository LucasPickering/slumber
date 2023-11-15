# Profile

A profile is a collection of static template values. It's useful for configuring and switching between multiple different environments/settings/etc.

Profiles also support nested templates, via the `!template` tag.

## Fields

| ≈      | Field                                                 | Type                               | Description   | Default |
| ------ | ----------------------------------------------------- | ---------------------------------- | ------------- | ------- |
| `id`   | `string`                                              | Unique identifier for this profile | Required      |
| `name` | `string`                                              | Descriptive name to use in the UI  | Value of `id` |
| `data` | [`mapping[string, ProfileValue]`](./profile_value.md) | Fields, mapped to their values     | `{}`          |

## Examples

```yaml
id: local
name: Local
data:
  host: localhost:5000
  url: !template "https://{{host}}"
  user_guid: abc123
```
