# Collection Reuse with `$ref`

Slumber supports a `$ref` anywhere in any YAML file that allows referencing any other part of a YAML document (including other files). It uses the [JSON Reference](https://datatracker.ietf.org/doc/html/draft-pbryan-zyp-json-ref-03) and [JSON Pointer](https://datatracker.ietf.org/doc/html/rfc6901) notation used by [OpenAPI](https://swagger.io/docs/specification/v3_0/using-ref/).

The format of the `$ref` is a [URI](https://datatracker.ietf.org/doc/html/rfc3986) with an optional base/path. The base can be:

- Empty, indicating a reference within the same file
- A file path, indicating a reference to another file
  - Path is always relative to **the importing file**

```yaml
requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"

  get_fish:
    method:
      $ref: "#/requests/list_fish/method"
    url: "{{ host }}/fishes/{{ fish_id }}"
```

The reference _source_ is everything before the `#`; the pointer is everything after.

## The Problem

Let's start with an example of something that sucks. Let's say you're making requests to a fish-themed JSON API, and it requires authentication. Gotta protect your fish! Your request collection might look like so:

```yaml
profiles:
  local:
    data:
      host: http://localhost:3000
      fish_id: 6
  production:
    data:
      host: https://myfishes.fish
      fish_id: 6

requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
    query:
      big: true
    headers:
      Accept: application/json
    authentication:
      type: bearer
      token: "{{ file('./api_token.txt') | trim() }}"

  get_fish:
    method: GET
    url: "{{ host }}/fishes/{{ fish_id }}"
    headers:
      Accept: application/json
    authentication:
      type: bearer
      token: "{{ file('./api_token.txt') | trim() }}"
```

## The Solution

You've heard of [DRY](https://en.wikipedia.org/wiki/Don%27t_repeat_yourself), so you know this is bad. Every profile has to include the fish ID, and every new request recipe has to copy-paste the authentication and headers.

You can easily reuse components of your collection using `$ref`:

```yaml
# The name here is arbitrary, pick any name you like. Make sure it starts with
# . to avoid errors about an unknown field
.base_profile_data:
  fish_id: 6
.base_request:
  headers:
    Accept: application/json
  authentication:
    type: bearer
    token: "{{ file('./api_token.txt') | trim() }}"

profiles:
  local:
    data:
      $ref: "#/.base_profile_data"
      host: http://localhost:3000
  production:
    data:
      $ref: "#/.base_profile_data"
      host: https://myfishes.fish

requests:
  list_fish:
    $ref: "#/.base_request"
    method: GET
    url: "{{ host }}/fishes"
    query:
      big: true

  get_fish:
    $ref: "#/.base_request"
    method: GET
    url: "{{ host }}/fishes/{{ fish_id }}"
```

Great! That's so much cleaner. Now each recipe can inherit whatever base properties you want just by including `$ref: "#/.base_request"`. This is still a bit repetitive, but it has the advantage of being explicit. You may have some requests that _don't_ want to include those values.

## Recursive Composition

But wait! What if you have a new request that needs an additional header? Unfortunately, `$ref` does not support recursive merging. If you need to extend the `headers` map from the base request, you'll need to pull the parent `headers` in manually:

```yaml
.base_request:
  headers:
    Accept: application/json
  authentication:
    type: bearer
    token: "{{ file('./api_token.txt') | trim() }}"

requests:
  create_fish:
    $ref: "#/.base_request"
    method: GET
    url: "{{ host }}/fishes/{{ fish_id }}"
    headers:
      $ref: "#/.base_request/headers"
      Host: myfishes.fish
    body:
      type: json
      data: { "kind": "barracuda", "name": "Barry" }
```

## Cross-File Composition

Reusing components within a single file is great and all, but `$ref` also supports importing components from other files:

**base.yml**

```yaml
requests:
  login:
    method: POST
    url: "{{ host }}/login"
    body:
      type: json
      data:
        {
          "username": "{{ prompt(message='Username') }}",
          "password": "{{ prompt(message='Password', sensitive=true) }}",
        }
```

**slumber.yml**

```yaml
requests:
  login:
    $ref: "./base.yml#/requests/login"
```

> Referenced files do _not_ need to be valid Slumber collections; any valid YAML file can be referenced

## Replacement vs Extension

Depending on how `$ref` is used, the referenced value will either replace or extend the reference.

- If `$ref` is the only field in its mapping, the entire mapping will be replaced
- If there are other fields in `$ref`, just the `$ref` _field_ will be replaced by the fields in the referenced mapping
  - In this case, the referenced value **must be a mapping**; any other value type will trigger an error

```yaml
refs:
  string: "hello!"
  mapping:
    a: 1
    b: 2

# `string`'s mapping value is replaced by the referenced string
# string: "hello!"
string:
  $ref: "#/refs/string"

# `mapping`'s mapping value is replaced by another mapping. This is functionally
# equivalent to extending `mapping` with `refs/mapping`.
#
# mapping:
#   a: 1
#   b: 2
mapping:
  $ref: "#/refs/mapping"

# The values of `refs/mapping` are replaced exactly where $ref is. `mapping/a`
# is overridden by `refs/mapping/a`, but `mapping/b` overrides `refs/mapping/b`
#
# mapping_extend:
#   a: 1
#   b: 3
mapping_extend:
  a: 0
  $ref: "#/refs/mapping"
  b: 3

# Error! Can't extend a mapping with a string
mapping_error:
  $ref: "#/refs/string"
  b: 3
```
