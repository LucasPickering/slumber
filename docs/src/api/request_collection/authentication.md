# Authentication

Authentication provides shortcuts for common HTTP authentication schemes. It populates the `authentication` field of a recipe. There are multiple source types, and the type is specified using [YAML's tag syntax](https://yaml.org/spec/1.2.2/#24-tags).

## Variants

| Variant   | Type                                            | Value                                                                                                          |
| --------- | ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `!basic`  | [`Basic Authentication`](#basic-authentication) | [Basic authentication](https://swagger.io/docs/specification/authentication/basic-authentication/) credentials |
| `!bearer` | `string`                                        | [Bearer token](https://swagger.io/docs/specification/authentication/bearer-authentication/)                    |

### Basic Authentication

Basic authentication contains a username and optional password.

| Field      | Type     | Description | Default  |
| ---------- | -------- | ----------- | -------- |
| `username` | `string` | Username    | Required |
| `password` | `string` | Password    | `""`     |

## Examples

```yaml
# Basic auth
requests:
  create_fish: !request
    method: POST
    url: "{{host}}/fishes"
    body: !json { "kind": "barracuda", "name": "Jimmy" }
    authentication: !basic
      username: user
      password: pass
---
# Bearer token auth
chains:
  token:
    source: !file
      path: ./token.txt
requests:
  create_fish: !request
    method: POST
    url: "{{host}}/fishes"
    body: !json { "kind": "barracuda", "name": "Jimmy" }
    authentication: !bearer "{{chains.token}}"
```
