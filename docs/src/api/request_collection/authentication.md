# Authentication

Authentication provides shortcuts for common HTTP authentication schemes. It populates the `authentication` field of a recipe. There are multiple source types, and the type is specified using the `type` field.

## Authentication Types

| Variant  | Value                                         |
| -------- | --------------------------------------------- |
| `basic`  | [Basic authentication](#basic-authentication) |
| `bearer` | [Bearer token](#bearer-token)                 |

### Basic Authentication

[Basic authentication](https://swagger.io/docs/specification/authentication/basic-authentication/) contains a username and optional password.

| Field      | Type     | Description | Default  |
| ---------- | -------- | ----------- | -------- |
| `username` | `string` | Username    | Required |
| `password` | `string` | Password    | `""`     |

### Bearer Token

[Bearer token authentication](https://swagger.io/docs/specification/authentication/bearer-authentication/) takes a single token.

| Field   | Type     | Description | Default  |
| ------- | -------- | ----------- | -------- |
| `token` | `string` | Token       | Required |

## Examples

```yaml
requests:
  basic_auth:
    method: GET
    url: "{{host}}/fishes"
    authentication:
      type: basic
      username: user
      password: "{{ prompt() }}"

  bearer_auth:
    method: GET
    url: "{{host}}/fishes"
    authentication:
      type: bearer
      token: "{{ file('token.txt') }}"
```
