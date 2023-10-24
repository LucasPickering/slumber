# Request Recipe

A request recipe defines how to make a particular request. For a REST API, you'll typically create one request recipe per endpoint.

## Fields

| Field     | Type                                                      | Description                       | Default       |
| --------- | --------------------------------------------------------- | --------------------------------- | ------------- |
| `id`      | `string`                                                  | Unique identifier for this recipe | Required      |
| `name`    | `string`                                                  | Descriptive name to use in the UI | Value of `id` |
| `method`  | [`TemplateString`](./template_string.md)                  | HTTP request method               | Required      |
| `url`     | [`TemplateString`](./template_string.md)                  | HTTP request URL                  | Required      |
| `query`   | [`mapping[string, TemplateString]`](./template_string.md) | HTTP request query parameters     | `{}`          |
| `headers` | [`mapping[string, TemplateString]`](./template_string.md) | HTTP request headers              | `{}`          |
| `body`    | [`TemplateString`](./template_string.md)                  | HTTP request body                 | `null`        |

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
