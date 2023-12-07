# Request Recipe

A request recipe defines how to make a particular request. For a REST API, you'll typically create one request recipe per endpoint.

## Fields

| Field     | Type                                         | Description                       | Default                |
| --------- | -------------------------------------------- | --------------------------------- | ---------------------- |
| `name`    | `string`                                     | Descriptive name to use in the UI | Value of key in parent |
| `method`  | `string`                                     | HTTP request method               | Required               |
| `url`     | [`Template`](./template.md)                  | HTTP request URL                  | Required               |
| `query`   | [`mapping[string, Template]`](./template.md) | HTTP request query parameters     | `{}`                   |
| `headers` | [`mapping[string, Template]`](./template.md) | HTTP request headers              | `{}`                   |
| `body`    | [`Template`](./template.md)                  | HTTP request body                 | `null`                 |

## Examples

```yaml
id: login
name: Login
method: POST
url: "{{host}}/anything/login"
headers:
  accept: application/json
  content-type: application/json
query:
  root_access: yes_please
body: |
  {
    "username": "{{chains.username}}",
    "password": "{{chains.password}}"
  }
```
