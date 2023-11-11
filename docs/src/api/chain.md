# Chain

A chain is a intermediate data type to enable complex template values. Chains enable complex value sources and additional customization, such as much values as sensitive to be masked in the UI.

To use a chain in a template, reference it as `{{chains.<id>}}`.

## Fields

| Field       | Type                                                                                   | Description                                             | Default  |
| ----------- | -------------------------------------------------------------------------------------- | ------------------------------------------------------- | -------- |
| `id`        | `string`                                                                               | Unique identifier for this chain                        | Required |
| `source`    | [`ChainSource`](./chain_source.md)                                                     | Source of the chained value                             | Required |
| `sensitive` | `boolean`                                                                              | Should the value be hidden in the UI?                   | `false`  |
| `selector`  | [`JSONPath`](https://www.ietf.org/archive/id/draft-goessner-dispatch-jsonpath-00.html) | Selector to narrow down results in a chained JSON value | `null`   |

See the [`ChainSource`](./chain_source.md) docs for more detail.

## Examples

```yaml
# Load chained value from a file
id: username
source: !file ./username.txt
---
# Prompt the user for a value whenever the request is made
id: password
source: !prompt Enter Password
sensitive: true
---
# Use a value from another response
# Assume the request recipe with ID `login` returns a body like `{"token": "foo"}`
id: auth_token
source: !request login
selector: $.token
```
