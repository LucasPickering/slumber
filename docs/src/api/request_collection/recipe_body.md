# Recipe Body

There are a variety of ways to define the body of your request. Slumber supports structured bodies for a fixed set of known content types (see table below).

In addition, you can pass any [`Template`](./template.md) to render any text or binary data. In this case, you'll probably want to explicitly set the `Content-Type` header to tell the server what kind of data you're sending. This may not be necessary though, depending on the server implementation.

## Body Types

The following content types have first-class support. Slumber will automatically set the `Content-Type` header to the specified value, but you can override this simply by providing your own value for the header.

| Variant            | Type                                         | `Content-Type`                      | Description                                                                                               |
| ------------------ | -------------------------------------------- | ----------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `!json`            | Any                                          | `application/json`                  | Structured JSON body; all strings are treated as templates                                                |
| `!form_urlencoded` | [`mapping[string, Template]`](./template.md) | `application/x-www-form-urlencoded` | URL-encoded form data; [see here for more](https://developer.mozilla.org/en-US/docs/Web/HTTP/Methods/POST |

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
    # Content-Type header will be set automatically based on the body type
    body: !json { "name": "Alfonso" }
```
