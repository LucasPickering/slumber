# Template

A template is represented in YAML as a normal string, and thus supports [all of YAML's string syntaxes](https://www.educative.io/answers/how-to-represent-strings-in-yaml). Templates receive post-processing that injects dynamic values into the string. A templated value is represented with `{{...}}`.

Templates can generally be used in any _value_ in a request recipe (_not_ in keys), as well as in profile values and chains. This makes them very powerful, because you can compose templates with complex transformations.

For more detail on usage and examples, see the [user guide page on templates](../../user_guide/templates.md).

## Template Sources

There are several ways of sourcing templating values:

| Source                        | Syntax                | Description                                                                                                              | Default          |
| ----------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------ | ---------------- |
| [Profile](./profile.md) Field | `{{field_name}}`      | Static value from a profile                                                                                              | Error if unknown |
| Environment Variable          | `{{env.VARIABLE}}`    | Environment variable from parent shell/process. **Deprecated in favor of the [`!env` chain source](./chain_source.md).** | `""`             |
| [Chain](./chain.md)           | `{{chains.chain_id}}` | Complex chained value                                                                                                    | Error if unknown |

## Examples

```yaml
# Profile value
"hello, {{location}}"
---
# Multiple dynamic values
"{{greeting}}, {{location}}"
---
# Environment variable
"hello, {{env.LOCATION}}"
---
# Chained value
"hello, {{chains.where_am_i}}"
---
# No dynamic values
"hello, world!"
```
