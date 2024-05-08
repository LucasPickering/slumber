# Chain

A chain is a intermediate data type to enable complex template values. Chains also provide additional customization, such as marking values as sensitive.

To use a chain in a template, reference it as `{{chains.<id>}}`.

## Fields

| Field          | Type                                                                                   | Description                                                                                                                            | Default  |
| -------------- | -------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| `source`       | [`ChainSource`](./chain_source.md)                                                     | Source of the chained value                                                                                                            | Required |
| `sensitive`    | `boolean`                                                                              | Should the value be hidden in the UI?                                                                                                  | `false`  |
| `selector`     | [`JSONPath`](https://www.ietf.org/archive/id/draft-goessner-dispatch-jsonpath-00.html) | Selector to transform/narrow down results in a chained value. See [Filtering & Querying](../../user_guide/filter_query.md)             | `null`   |
| `content_type` | [`ContentType`](./content_type.md)                                                     | Force content type. Not required for `request` and `file` chains, as long as the `Content-Type` header/file extension matches the data |          |
| `trim`         | [`ChainOutputTrim`](#chain-output-trim)                                                | Trim whitespace from the rendered output                                                                                               | `none`   |

See the [`ChainSource`](./chain_source.md) docs for detail on the different types of chainable values.

## Chain Output Trim

This defines how leading/trailing whitespace should be trimmed from the resolved output of a chain.

| Variant | Description                               |
| ------- | ----------------------------------------- |
| `none`  | Do not modify the resolved string         |
| `start` | Trim from just the start of the string    |
| `end`   | Trim from just the end of the string      |
| `both`  | Trim from the start and end of the string |

## Examples

```yaml
# Load chained value from a file
username:
  source: !file
    path: ./username.txt
---
# Prompt the user for a value whenever the request is made
password:
  source: !prompt
    message: Enter Password
  sensitive: true
---
# Use a value from another response
# Assume the request recipe with ID `login` returns a body like `{"token": "foo"}`
auth_token:
  source: !request
    recipe: login
  selector: $.token
---
# Use the output of an external command
username:
  source: !command
    command: [whoami]
    trim: both # Shell commands often include an unwanted trailing newline
```
