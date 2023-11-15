# Profile Value

A profile value is the value associated with a particular key in a profile. Typically profile values are just simple strings, but they can also be other variants.

In the case of a nested template, the inner template will be rendered into its own value, then injected into the outer string.

## Variants

| Variant    | Type                        | Description                                   |
| ---------- | --------------------------- | --------------------------------------------- |
| `raw`      | `string`                    | Static string (key can optionally be omitted) |
| `template` | [`Template`](./template.md) | Nested template, to be rendered inline        |

## Examples

```yaml
!raw http://localhost:5000
---
# The !raw key is the default, and can be omitted
http://localhost:5000
---
!template http://{{hostname}}
```
