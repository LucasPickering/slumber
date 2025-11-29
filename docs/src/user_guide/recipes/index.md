# Recipes

Recipes are the core feature of Slumber; they define how to generate HTTP requests. The terms "recipe" and "request" are often used interchangeably by users, but there is a technical distinction:

- A recipe is a definition for how to generate any number of requests
- A request is a single chunk of data (URL+headers+body) to send to a server

The distinction isn't that important; generally it's easy to figure out what "request" means based on the context. This is exactly why Slumber uses a `requests` field in the collection file instead of `recipes`. It's easy to guess and easy to remember.

## Method & URL

A recipe's [HTTP method](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Methods) is defined by the `method` field. Unlike other recipe fields, `method` is **not** a template. It must be a static string containing one of the supported methods (case insensitive):

- `CONNECT`
- `DELETE`
- `GET`
- `HEAD`
- `OPTIONS`
- `PATCH`
- `POST`
- `PUT`
- `TRACE`

The recipe URL is defined by the `url` field:

```yaml
requests:
  get_fishes:
    method: GET
    url: "https://myfishes.fish/fishes"
```

## Query Parameters

> See the [API docs](../../api/request_collection/query_parameters.md) for more detailed info.

Query parameters are specified via the `query` field. They form a component of a request URL and provide additional information to the server about a request. In a request recipe, query parameters are defined as a map of `parameter: value`. The value can be a singular value (string/boolean/etc.) or a list of values.

```yaml
recipes:
  get_fishes:
    method: GET
    url: "https://myfishes.fish/fishes"
    query:
      big: true
      color: [red, blue] # This parameter has multiple values
      name: "Barry"
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
    url: "https://myfishes.fish/fishes"
    headers:
      X-Custom-Header: "You are https://myfishes.fish"
```

> Before manually specifying headers, read the sections below on [authentication](#authentication) and [request bodies](#body). Slumber has first-class support for common request features that may make it unnecessary to specify headers such as `Content-Type` or `Authorization`.

## Authentication

> See the [API docs](../../api/request_collection/authentication.md) for more detailed info.

Slumber supports multiple methods of request authentication, making it easier to build request with common authentication schemes. The supported types currently are:

- Basic (username/password)
- Bearer (API token)

If you'd like support for a new authentication scheme, please [file an issue](https://github.com/LucasPickering/slumber/issues/new).

```yaml
requests:
  basic_auth:
    method: GET
    url: "https://myfishes.fish/fishes"
    authentication:
      type: basic
      username: user
      password: hunter2

  bearer_auth:
    method: GET
    url: "https://myfishes.fish/fishes"
    authentication:
      type: bearer
      token: my-token
```

## Body

[See the next page](./bodies.md)
