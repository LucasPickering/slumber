# v3 to v4 Migration

Slumber 4.0 introduced a [set of breaking changes](https://github.com/LucasPickering/slumber/releases/tag/v4.0.0) to the collection format, requiring migration of your collection files to the new format. You have two options to migrate automatically:

**CLI Migration Tool**

You can upgrade with the migration tool included in the v4 CLI:

```sh
slumber import v3 <old file> <new file>
```

The generated collection will be _functionally_ equivalent to the old collection, but due to limitations in the Rust YAML ecosystem, non-semantic content such as whitespace, comments, and anchors/aliases will be lost. Depending on the complexity of your collection, this migration may be 100% of what you need, or it may provide a good starting point for you to finish cleaning up by hand. You can also use an LLM to do the touch-ups, but in that case it may just be easier to do the entire migration via LLM (see next section).

> "Functionally equivalent" means loading the TUI on the new collection should show you the exact same set of profiles/requests that you saw in v3. If this isn't the case (e.g. if a new template doesn't function the same as it did before), please [file a bug](https://github.com/lucaspickering/slumber/issues/new)!

**LLM Migration**

If you're open to using an LLM, you can provide it this guide and it should generate a v4 collection that is functionally equivalent, retain comments and deduplicate content via `$ref`. Provide it this prompt:

```
Migrate this file from Slumber v3 to Slumber v4 using this guide:
https://slumber.lucaspickering.me/book/other/v4_migration.html
```

> Some AI Agents won't follow URLs or only fetch snippets of the page rather than the entire content. If your new collection doesn't work, try copying the entire page content below into the prompt.

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

The v3 chain sources (and their parameters) have each been replaced by a corresponding function.

> In the new template language, required arguments are passed as positional arguments `file("my/path")` while optional arguments are passed as keywords `prompt(default="Default")`. These examples include all optional arguments for completeness, but they can be omitted if not needed.

**`!command` becomes [`command`](../api/template_functions.md#command)**

```yaml
source: !command
  command: ["echo", "test"]
  stdin: "{{host}}"
# becomes
command(["echo", "test"], stdin=host)
```

**`!env` becomes [`env`](../api/template_functions.md#env)**

```yaml
source: !env
  variable: MY_VAR
# becomes
env("MY_VAR")
```

**`!file` becomes [`file`](../api/template_functions.md#file)**

```yaml
source: !file
  path: my/file
# becomes
file("my/file")
```

**`!prompt` becomes [`prompt`](../api/template_functions.md#prompt)**

```yaml
source: !prompt
  message: "Enter data"
  default: "Default"
sensitive: true
# becomes
prompt(message="Enter data", default="Default", sensitive=true)
```

**`!request` split into [`response`](../api/template_functions.md#response) and [`response_header`](../api/template_functions.md#response_header)**

```yaml
source: !request
  recipe: "login"
  trigger: !expire 12h
  section: !body
# becomes
response("login", trigger="12h")
```

```yaml
source: !request
  recipe: "login"
  trigger: !expire 12h
  section: !header Content-Type
# becomes
response_header("login", "Content-Type", trigger="12h")
```

**`!select` becomes [`select`](../api/template_functions.md#select)**

```yaml
source: !select
  options:
    - option1
    - option2
  message: "Message"
# becomes
select(options=["option1", "option2"], message="Message")
```

In addition, the following chain fields have been replaced by utility functions:

- `selector` and `selector_mode` -> [`jsonpath`](../api/template_functions.md#jsonpath)
- `sensitive` -> [`sensitive`](../api/template_functions.md#sensitive)
- `trim` -> [`trim`](../api/template_functions.md#trim)
- `content_type` is no longer needed, as only JSON was ever supported

These functions can be used via the pipe operator:

```yaml
source: !file
  path: "file.json"
trim: both
selector: "$.items"
selector_mode: array
sensitive: true
# becomes
file("file.json") | trim(mode="both") | jsonpath("$.items", mode="array") | sensitive()
```

Finally, there is a function `concat()` that can be used to replicate the behavior of nested templates within chain config:

```yaml
chains:
  file_name:
    source: !prompt
      message: File name
  path:
    source: !file
      path: "data/{{file_name}}.json"
# becomes
"{{ file(concat(['data/', prompt(message='File name'), '.json']))"
```

> `concat()` takes a single array argument!

This is only necessary if you have nested templates **with multiple components**. In other words, if you do this conversion but the array passed to `concat()` only has one element, you can just omit the `concat()`.

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
