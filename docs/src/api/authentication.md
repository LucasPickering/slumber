# Authentication

Authentication provides shortcuts for common HTTP authentication schemes. It populates the `authentication` field of a recipe. There are multiple source types, and the type is specified using [YAML's tag syntax](https://yaml.org/spec/1.2.2/#24-tags).

## Variants

| Variant  | Type                                            | Value                                                                                                          |
| -------- | ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `basic`  | [`Basic Authentication`](#basic-authentication) | [Basic authentication](https://swagger.io/docs/specification/authentication/basic-authentication/) credentials |
| `bearer` | `string`                                        | [Bearer token](https://swagger.io/docs/specification/authentication/bearer-authentication/)                    |

### Basic Authentication

Basic authentication contains a username and optional password.

| Field      | Type     | Description | Default  |
| ---------- | -------- | ----------- | -------- |
| `username` | `string` | Username    | Required |
| `password` | `string` | Password    | `""`     |

## Examples

```yaml
!basic
username: user
password: pass
---
!bearer 4J2e0TYqKA3gFllfTu17OF7n8g1CeAxZyi/MK5g40/o=
```
