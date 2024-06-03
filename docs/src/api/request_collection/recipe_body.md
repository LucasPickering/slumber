# Recipe Body

There are a variety of ways to define the body of your request. Slumber supports structured bodies for a fixed set of known content types (see table below). In addition, you can pass any [`Template`](./template.md) to render any text or binary data. This may not be necessary though, depending on the server implementation.

## Supported Content Types

The following content types have first-class support. All other bodies types must be specified as raw text/binary.

| Variant | Type | Description                                                      |
| ------- | ---- | ---------------------------------------------------------------- |
| `!json` | Any  | Structured JSON body, where all strings are treated as templates |

> Note: Unlike some other HTTP clients, Slumber does **not** automatically set the `Content-Type` header for you. In general you'll want to include that in your request recipe, to tell the server the type of the content you're sending. While this may be inconvenient, it's not possible for Slumber to always know the correct header value, and Slumber's design generally prefers explicitness over convenience.

## Examples

```yaml
chains:
  image:
    source: !file
      path: ./fish.png

requests:
  text_body: !request
    method: POST
    url: "{{host}}/fishes/{{fish_id}}/name"
    headers:
      Content-Type: text/plain
    body: Alfonso

  binary_body: !request
    method: POST
    url: "{{host}}/fishes/{{fish_id}}/image"
    headers:
      Content-Type: image/jpg
    body: "{{chains.fish_image}}"

  json_body: !request
    method: POST
    url: "{{host}}/fishes/{{fish_id}}"
    headers:
      Content-Type: application/json
    body: !json { "id": "{{fish_id}}", "name": "Alfonso" }

  # This recipe is equivalent to `json_body`. The `!json` syntax is just
  # convenience to make it easier to write and structure your bodies
  json_body_raw: !request
    method: POST
    url: "{{host}}/fishes/{{fish_id}}"
    headers:
      Content-Type: application/json
    body: '{"id": "{{fish_id}}", "name": "Alfonso"}'
```
