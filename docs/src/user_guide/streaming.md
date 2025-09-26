# Data Streaming & File Upload

If you want to generate HTTP requests with very large bodies, you may want to use streams to upload

In addition to reducing memory usage and saving the TUI from having to load and display a giant body, streaming also enables some additional features for [multipart form bodies](../api/request_collection/recipe_body.md#multipart-form).

## What is Streaming?

Streaming is when the HTTP client sends bytes directly from a source such as a file to the HTTP server without loading the entire body into memory. It's useful when the body is very large because it saves time and memory.

## Streaming in Slumber

Slumber supports streaming in these contexts:

- `stream` request body
- `form_multipart` request body fields

and from these functions:

- [command](../api/template_functions.md#command)
- [file](../api/template_functions.md#file)

```yaml
# These bodies **WILL** be streamed
file:
  method: POST
  url: "{{ host }}/upload"
  body:
    type: stream
    data: "{{ file('image.png') }}"

command:
  method: POST
  url: "{{ host }}/upload"
  body:
    type: stream
    data: "here's some bytes: {{ command(['head', '-c', '1000', '/dev/random']) }}"

multipart:
  method: POST
  url: "{{ host }}/upload"
  body:
    type: form_multipart
    data:
      image: "{{ file('./image.png') }}"
```

```yaml
# These bodies will **NOT** be streamed
file:
  method: POST
  url: "{{ host }}/upload"
  # The template contains multiple chunks, so it can't be streamed
  body: "{{ file('image.png') }}"
```

### Multipart File Streaming

In addition to support for general streaming of bytes, `form_multipart` fields also have special support for file uploads. If the value of a field is a **template with a single chunk**, and the final call of the chunk is to `file()`, then the fille will be uploaded directly. This has two effects on that part of the form:

- The `Content-Type` header will be set based on the file extension
- The `Content-Disposition` header will have the `filename` field set

Here's an example:

```yaml
multipart_file:
  method: POST
  url: "{{ host }}/upload"
  body:
    type: form_multipart
    data:
      image: "{{ file('./data/data.json') }}"
```

This will generate a request body like:

```
--BOUNDARY
Content-Disposition: form-data; name="file"; filename="data.json"
Content-Type: application/json

{ "a": 1, "b": 2 }
--BOUNDARY--
```

But if you generate the same body with an equivalent `command()` call, the body **will still be streamed**, however the headers will not be set based on the file path.

```yaml
multipart_command:
  method: POST
  url: "{{ host }}/upload"
  body:
    type: form_multipart
    data:
      image: "{{ command(['cat', './data/data.json']) }}"
```

This will generate a request body like:

```
--BOUNDARY
Content-Disposition: form-data; name="file"

{ "a": 1, "b": 2 }
--BOUNDARY--
```
