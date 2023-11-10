# Chain Source

A chain source defines how a [Chain](./chain.md) gets its value. It populates the `source` field of a chain. There are multiple source types, and the type is specified using [YAML's tag syntax](https://yaml.org/spec/1.2.2/#24-tags).

## Types

| Type      | Type       | Value                                | Chained Value                                                   |
| --------- | ---------- | ------------------------------------ | --------------------------------------------------------------- |
| `request` | `string`   | Request Recipe ID                    | Body of the most recent response for a specific request recipe. |
| `command` | `string[]` | `[program, ...arguments]`            | Stdout of the executed command                                  |
| `file`    | `string`   | Path (relative to current directory) | Contents of the file                                            |
| `prompt`  | `string`   | Descriptive prompt for the user      | Value entered by the user                                       |

## Examples

See the [`Chain`](./chain.md) docs for more holistic examples.

```yaml
!request login
---
!command ["echo", "-n", "hello"]
---
!file ./username.txt
---
!prompt Enter Password
```
