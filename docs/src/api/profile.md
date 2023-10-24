# Profile

A profile is a collection of static template values. It's useful for configuring and switching between multiple different environments/settings/etc.

## Fields

| Field  | Type                      | Description                           | Default       |
| ------ | ------------------------- | ------------------------------------- | ------------- |
| `id`   | `string`                  | Unique identifier for this profile    | Required      |
| `name` | `string`                  | Descriptive name to use in the UI     | Value of `id` |
| `data` | `mapping[string, string]` | Fields, mapped to their static values | `{}`          |

## Examples

```yaml
id: local
name: Local
data:
  host: http://localhost:5000
  user_guid: abc123
```
