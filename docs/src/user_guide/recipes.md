# Recipes

Recipes are the core feature of Slumber; they define how to generate HTTP requests. The terms "recipe" and "request" are often used interchangeably by users and documentation alike, but there is a technical distinction:

- A recipe is a definition for how to generate any number of requests
- A request is a single chunk of data (URL+headers+body) to send to a server

The distinction isn't that important; generally it's easy to figure out what "request" means based on the context. This is exactly why Slumber uses a `requests` field in the collection file instead of `recipes`. It's easy to guess and easy to remember.

## Method & URL

A request's [HTTP method](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Methods) is defined by the `method` field. Unlike other request fields, `method` is **not** a template. It must be a static string containing one of the supported methods:

- `CONNECT`
- `DELETE`
- `GET`
- `HEAD`
- `OPTIONS`
- `PATCH`
- `POST`
- `PUT`
- `TRACE`

The request URL is defined by the `url` field and _can_ be a template.

```yaml
profiles:
  default:
    data:
      host: https://myfishes.fish

requests:
  get_fishes:
    method: GET
    url: "{{ host }}/fishes"
```

## Query Parameters

> See the [API docs](../api/request_collection/query_parameters.md) for more detailed info.

Query parameters are specified via the `query` field. They form a component of a request URL and provide additional information to the server about a request. In a request recipe, query parameters are defined as a map of `parameter: value`. The value can be a singular template (string/boolean/etc.) or a list of values.

```yaml
profiles:
  default:
    data:
      host: https://myfishes.fish
      name: Barry

recipes:
  get_fishes:
    method: GET
    url: "{{ host }}/fishes"
    query:
      big: true
      color: [red, blue] # This parameter has multiple values
      name: "{{ name }}"
```

This will generate the URL `https://myfishes.fish/fishes?big=true&color=red&color=blue&name=Barry`.

## Headers

[HTTP request headers](https://developer.mozilla.org/en-US/docs/Glossary/Request_header) are specified via the `headers` field, which is a mapping of `{header: value}`. The keys (header names) must be static, but values can be templated. Typically header values are UTF-8 text, but can be any arbitrary stream of bytes compliant with the HTTP spec.

```yaml
profiles:
  default:
    data:
      host: https://myfishes.fish

recipes:
  get_fishes:
    method: GET
    url: "{{ host }}/fishes"
    headers:
      X-Custom-Header: "You are {{ host }}"
```

> Before manually specifying headers, read the sections below on [authentication](#authentication) and [request bodies](#body). Slumber has first-class support for common request features that may make it unnecessary to specify headers such as `Content-Type` or `Authorization`.

## Authentication

> See the [API docs](../api/request_collection/authentication.md) for more detailed info.

Slumber supports multiple methods of request authentication, making it easier to build request with common authentication schemes. The supported types currently are:

- Basic (username/password)
- Bearer (API token)

If you'd like support for a new authentication scheme, please [file an issue](https://github.com/LucasPickering/slumber/issues/new).

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

## Body

> See the [API docs](../api/request_collection/recipe_body.md) for more detailed info.

Slumber supports a number of different body types:

- Raw text/bytes
- JSON
- URL-encoded forms (`application/x-www-form-urlencoded`)
- Multipart forms (`multipart/form-data`)

Here's an example of each one in practice:

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

### Non-string JSON templates

JSON bodies support dynamic non-string values. By using a template with a single dynamic chunk (i.e. there's no next content, just a `{{ ... }}`), you can create non-string values. Let's say we have a JSON file `./friends.json` with this content:

```json
["Barry", "Dora"]
```

We can use this file in a request body:

```yaml
requests:
  json_body:
    method: POST
    url: "{{ host }}/fishes/{{ fish_id }}"
    body:
      type: json
      data:
        {
          "name": "Alfonso",
          "friends": "{{ file('./friends.json') | json() }}",
        }
```

The request body will render as:

```json
{
  "name": "Alfonso",
  "friends": ["Barry", "Dora"]
}
```

A few things to notice here:

- We had to explicitly parse the contents of the file with `json()`. By default the content loaded is just artbirary bytes; Slumber doesn't know it's supposed to be JSON.
- The parsed JSON is included directly into the JSON body, _without_ the surrounding quotes from the template. In other words, the value was **unpacked**.

In some cases this behavior may not be desired, e.g. when combined with `jsonpath()`. You can pipe to `string()` to **disable this behavior**:

```yaml
requests:
  json_body:
    method: POST
    url: "{{ host }}/fishes/{{ fish_id }}"
    body:
      type: json
      data:
        {
          "name": "Alfonso",
          "friends": "{{ file('./friends.json') | jsonpath('$[*]') | string() }}",
        }
```

This will render to:

```json
{
  "name": "Alfonso",
  "friends": "[\"Barry\", \"Dora\"]"
}
```
