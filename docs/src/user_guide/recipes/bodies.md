# Bodies

> See the [API docs](../../api/request_collection/recipe_body.md) for more detailed info.

Slumber supports a number of different body types:

- Raw text/bytes
- JSON
- URL-encoded forms (`application/x-www-form-urlencoded`)
- Multipart forms (`multipart/form-data`)

## Raw Text/Bytes

Passing just a template to the `body` field gives you a raw string/binary body.

```yaml
text_body:
  method: POST
  url: "https://myfishes.fish/fishes/42/name"
  headers:
    Content-Type: text/plain
  body: Alfonso
```

```yaml
binary_body:
  method: POST
  url: "https://myfishes.fish/fishes/42/image"
  headers:
    Content-Type: image/jpg
  body: "{{ file('./fish.png') }}"
```

## JSON

`type: json` allows you to pass arbitrary values to the `data` field. The string values are all treated as templates.

```yaml
json_body:
  method: POST
  url: "https://myfishes.fish/fishes/42"
  # Content-Type header will be set to `application/json` automatically
  body:
    type: json
    data: { "name": "{{ name }}" }
```

See [this example](../templates/examples.md#non-string-json-templates) for how to use dynamic non-string values in JSON bodies. This is called "unpacking".

## URL-encoded Form

`type: form_urlencoded` expects a key-value mapping for the `data` field. Each entry is a field in the form. The values are all templates.

```yaml
urlencoded_body:
  method: POST
  url: "https://myfishes.fish/fishes/42"
  # Content-Type header will be set to `application/x-www-form-urlencoded` automatically
  body:
    type: form_urlencoded
    data:
      name: Alfonso
```

## Multipart Form

`type: form_urlencoded` expects a key-value mapping for the `data` field. Each entry is a field in the form. The values are all templates, and can be either text or binary values.

```yaml
multipart_body:
  method: POST
  url: "https://myfishes.fish/fishes/42"
  # Content-Type header will be set to `multipart/form-data` automatically
  body:
    type: form_multipart
    data:
      name: Alfonso
      image: b"\x12\x34"
```
