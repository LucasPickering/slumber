# Chain

A chain is a intermediate data type to enable complex template values. Chains also provide additional customization, such as marking values as sensitive.

To use a chain in a template, reference it as `{{chains.<id>}}`.

## Fields

| Field           | Type                                                                                   | Description                                                                                                                                                                                          | Default  |
| --------------- | -------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| `source`        | [`ChainSource`](./chain_source.md)                                                     | Source of the chained value                                                                                                                                                                          | Required |
| `sensitive`     | `boolean`                                                                              | Should the value be hidden in the UI?                                                                                                                                                                | `false`  |
| `selector`      | [`JSONPath`](https://www.ietf.org/archive/id/draft-goessner-dispatch-jsonpath-00.html) | Selector to transform/narrow down results in a chained value. See [Filtering & Querying](../../user_guide/filter_query.md)                                                                           | `null`   |
| `selector_mode` | [`SelectorMode`](#selector-mode)                                                       | Control selector behavior when query returns multiple results                                                                                                                                        | `auto`   |
| `content_type`  | `string`                                                                               | Force content type. Not required for `request` and `file` chains, as long as the `Content-Type` header/file extension matches the data. See [here](./content_type.md) for a list of supported types. |          |
| `trim`          | [`ChainOutputTrim`](#chain-output-trim)                                                | Trim whitespace from the rendered output                                                                                                                                                             | `none`   |

See the [`ChainSource`](./chain_source.md) docs for detail on the different types of chainable values.

## Chain Output Trim

This defines how leading/trailing whitespace should be trimmed from the resolved output of a chain.

| Variant | Description                               |
| ------- | ----------------------------------------- |
| `none`  | Do not modify the resolved string         |
| `start` | Trim from just the start of the string    |
| `end`   | Trim from just the end of the string      |
| `both`  | Trim from the start and end of the string |

## Selector Mode

The selector mode controls how Slumber handles returns JSONPath query results from the `selector` field, relative to how many matches the query returned. The table below shows how each mode behaves for a query that produces no values (`$.id`) a single value (`$[0].name`) vs multiple values (`$[*].name`) for this example data:

```json
[{ "name": "Apple" }, { "name": "Kiwi" }, { "name": "Mango" }]
```

| Variant  | Description                                                                       | `$.id` | `$[0].name` | `$[*].name`                  |
| -------- | --------------------------------------------------------------------------------- | ------ | ----------- | ---------------------------- |
| `auto`   | If query returns a single value, use it. If it returns multiple, use a JSON array | Error  | `Apple`     | `["Apple", "Kiwi", "Mango"]` |
| `single` | If a query returns a single value, use it. Otherwise, error.                      | Error  | `Apple`     | Error                        |
| `array`  | Return results as an array, regardless of count.                                  | `[]`   | `["Apple"]` | `["Apple", "Kiwi", "Mango"]` |

The default selector mode is `auto`.

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
# Prompt the user to select a value from a static list
fruit:
  souce: !select
    message: Select Fruit
    options:
      - apple
      - banana
      - guava
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
