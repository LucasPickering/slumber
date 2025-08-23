# Recipe Body

There are a variety of ways to define the body of your request. Slumber supports structured bodies for a fixed set of known content types (see table below). In addition to handling body serialization for you, structured bodies will also set the `Content-Type` header.

In addition, you can pass any [`Template`](../../user_guide/templates/index.md) to render any text or binary data. In this case, you'll probably want to explicitly set the `Content-Type` header to tell the server what kind of data you're sending. This may not be necessary though, depending on the server implementation.

## Body Types

The following content types have first-class support. Slumber will automatically set the `Content-Type` header to the specified value, but you can override this simply by providing your own value for the header.

| Variant           | `Content-Type`                      | Description                                                                                                |
| ----------------- | ----------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `json`            | `application/json`                  | Structured JSON body; all strings are treated as templates                                                 |
| `form_urlencoded` | `application/x-www-form-urlencoded` | URL-encoded form data; [see here for more](https://developer.mozilla.org/en-US/docs/Web/HTTP/Methods/POST) |
| `form_multipart`  | `multipart/form-data`               | Binary form data; [see here for more](https://developer.mozilla.org/en-US/docs/Web/HTTP/Methods/POST)      |

### JSON

JSON bodies can contain any data. All strings in the JSON are treated as [templates](../../user_guide/templates/index.md).

| Field  | Type | Description  | Default  |
| ------ | ---- | ------------ | -------- |
| `data` | Any  | JSON content | Required |

See [the guide](../../user_guide/recipes.md#body) for more detail on how to use JSON bodies.

### URL-encoded Form

[URL forms](https://developer.mozilla.org/en-US/docs/Web/HTTP/Methods/POST) can only pass text data.

| Field  | Type                                                                | Description | Default  |
| ------ | ------------------------------------------------------------------- | ----------- | -------- |
| `data` | [`mapping[string, Template]`](../../user_guide/templates/index.md)` | Form fields | Required |

See [the guide](../../user_guide/recipes.md#body) for more detail on how to use form bodies.

### Multipart Form

[Multipart forms](https://developer.mozilla.org/en-US/docs/Web/HTTP/Methods/POST) can pass text or binary data.

| Field  | Type                                                                | Description | Default  |
| ------ | ------------------------------------------------------------------- | ----------- | -------- |
| `data` | [`mapping[string, Template]`](../../user_guide/templates/index.md)` | Form fields | Required |

See [the guide](../../user_guide/recipes.md#body) for more detail on how to use form bodies.

## Examples

```yaml
requests:
  text_body:
    method: POST
    url: "{{ host }}/fishes/{{ fish_id }}/name"
    headers:
      Content-Type: text/plain
    body: Alfonso

  binary_body:
    method: POST
    url: "{{ host }}/fishes/{{ fish_id }}/image"
    headers:
      Content-Type: image/jpg
    body: "{{ file('./fish.png') }}"

  json_body:
    method: POST
    url: "{{ host }}/fishes/{{ fish_id }}"
    # Content-Type header will be set automatically based on the body type
    body:
      type: json
      data: { "name": "Alfonso" }

  urlencoded_body:
    method: POST
    url: "{{ host }}/fishes/{{ fish_id }}"
    # Content-Type header will be set automatically based on the body type
    body:
      type: form_urlencoded
      data:
        name: Alfonso

  multipart_body:
    method: POST
    url: "{{ host }}/fishes/{{ fish_id }}"
    # Content-Type header will be set automatically based on the body type
    body:
      type: form_multipart
      data:
        name: Alfonso
        image: "{{ file('./fish.png') }}"
```
