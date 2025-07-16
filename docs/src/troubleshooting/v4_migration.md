# v3 to v4 Migration

Slumber 4.0 introduced a [set of breaking changes](https://github.com/LucasPickering/slumber/releases/tag/v4.0.0) to the collection format, requiring migration of your collection files to the new format. Migration is simple using the included importer:

```sh
slumber import v3 <old file> <new file>
```

The new collection _should_ be equivalent to the old one, but you should keep your old version around just in case something broke. If you notice any differences, please [file a bug!](https://github.com/lucaspickering/slumber/issues/new).

## Manual Migration

If you prefer to do the import manually, or if the automatic importer isn't working for you, here's what needs to be changed:

### Rewrite Templates

Rewrite templates to use the new function-based syntax:

- Replace each chain type with its [equivalent function call](../api/template_functions.md)
- For chains used multiple times, deduplicate the migrated template expression using [common profile fields](../user_guide/templates/examples.md#deduplicating-template-expressions)

**Example**

```yaml
# v3
chains:
  file:
    source: !file
      path: "./data.txt"
    trim: both
  response:
    source: !request
      recipe: login
      selector: $.token

requests: !request
  r1:
    query:
      file: "{{chains.file}}"
      response: "{{chains.response}}"

# v4
requests:
  r1:
    query:
      file: "{{ file('./data.txt') | trim() }}"
      response: "{{ response('login') | jsonpath('$.token') }}"
```

The v3 chain sources map to functions like so:

- `!command` -> [`command`](../api/template_functions.md#command)
- `!env` -> [`env`](../api/template_functions.md#env)
- `!file` -> [`file`](../api/template_functions.md#file)
- `!prompt` -> [`prompt`](../api/template_functions.md#prompt)
- `!request` -> [`response`](../api/template_functions.md#response) or [`response_header`](../api/template_functions.md#response_header), depending on the `section` field
- `!select` -> [`select`](../api/template_functions.md#select)

In addition, the following chain fields have been replaced by utility functions:

- `selector` and `selector_mode` -> [`jsonpath`](../api/template_functions.md#jsonpath)
- `sensitive` -> [`sensitive`](../api/template_functions.md#sensitive)
- `trim` -> [`trim`](../api/template_functions.md#trim)
- `content_type` is no longer needed, as only JSON was ever supported

### Replace Anchors with `$ref`

The previous YAML anchor/alias and spread syntax has been replaced by a more powerful `$ref` syntax, akin to [OpenAPI](https://swagger.io/docs/specification/v3_0/using-ref/).

```yaml
# v3
.base_request: &base_request
  method: GET
  headers:
    Accept: application/json

requests:
  r1: !request
    <<: *base_request
    url: "https://myfishes.fish/fishes"

# v4
.base_request:
  method: GET
  headers:
    Accept: application/json

requests:
  r1:
    $ref: "#/.base_request"
    url: "https://myfishes.fish/fishes"
```

> Same as `<<:`, `$ref` does _not_ do a deep merge of objects. In the above example, any recipe containing a `headers` field would entirely overwrite `.base_request/headers`

See [Collection Reuse & Composition](../user_guide/composition.md) to learn more about the power of `$ref`, including sharing collection components across files.

### Remove `!tag` Syntax

Slumber no longer uses YAML `!tag` syntax.

- Remove `!request` and `!folder` entirely (they're now detected automatically)
- Authentication: `!basic` and `!bearer` have been replaced by `authentication.type`:

```yaml
# v3
authentication: !basic
  username: user1
  password: hunter2
authentication: !bearer token123

# v4
authentication:
  type: basic
  username: user1
  password: hunter2
authentication:
  type: bearer
  token: token123
```

- Body: `!json`, `!form_urlencoded`, and `!form_multipart` have been replaced by `type` and `data` fields:

```yaml
# v3
body: !json
  key: value
body: !form_urlencoded
  field1: value
body: !form_multipart
  field1: value

# v4
body:
  type: json
  data:
    key: value
body:
  type: form_urlencoded
  data:
    field1: value
body:
  type: form_multipart
  data:
    field1: value
```

### Query Parameters

Previously, query parameters could be expressed as either a mapping of `param: value` OR a sequence of strings `"param=value"`. This enabled the use of multiple instances of the same parameter. Query parameters must be mappings now, but support both a singular OR sequence value to enable multiple values for the same parameter.

**Any `query` blocks already in the `param: value` format do not need to be changed**.

For any `query` blocks in the `"param=value"` format, update them like so:

```yaml
# v3
query:
  - param1=value1
  - param1=value2
  - param2=value7

# v4
query:
  param1: [value1, value2]
  param2: value7
```
