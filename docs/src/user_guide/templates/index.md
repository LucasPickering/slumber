# Templates

Templates enable dynamic request construction. Slumber's template language is relatively simple when compared to HTML templating languages such as Handlebars or Jinja. The goal is to be simple, intuitive, and unsurprising. Every value in a request (except for the HTTP method) is a template, meaning it can be computed dynamically.

## Quick Start

Slumber templates in 60 seconds or less:

- Double curly braces `{{...}}` denotes a dynamic value in a template
- Literals:
  - Null: `null`
  - Booleans: `true` and `false`
  - Integers: `-3` or `1000`
  - Floats: `-3.14`, `1000.0`, `3.14e2`
  - String: `'hello'` or `"hello"` (single or double quotes)
    - Escape inner quotes with `\`
  - Bytes: `b'hello'` or `b"hello"`
  - Array: `[1, "hello", [true, b"world"]]`
  - Object: `{ 'a': 1, 'b': 2 }`
- Profile fields: `host` (see [Profiles](../profiles.md))
- Function calls: `g(f(), 1)`
  - [See all available functions](../../api/template_functions.md)
- Pipes: `f() | g(1)`
  - Result of `f()` is passed as the _last_ argument to `g`
  - `f() | g(1)` is equivalent to `g(1, f())`

Put it all together and you can build collections like this:

```python
{{ host }}/fish/{{ response('list_fish') | jsonpath('$[0].id') }}
```

If you still have questions, you can keep reading or [skip to some examples](./examples.md).

## YAML Syntax

Templates are defined as strings in your request collection YAML file. For example, here's a template for a request URL:

```yaml
requests:
  list_fish:
    method: GET
    url: "{{ host }}/fish"
```

Most values in a request collection (e.g. URL, request body, etc.) are templates. [Even profile values are templates!](../profiles.md#dynamic-profile-values) Map keys (e.g. recipe ID, profile ID) are _not_ templates; they must be static strings.

> **A note on YAML string syntax**
>
> One of the advantages (and disadvantages) of YAML is that it has a number of different string syntaxes. This enables you to customize your templates according to your specific needs around the behavior of whitespace and newlines. **In most cases, you should just use `""` on all strings.** See [YAML's string syntaxes](https://www.educative.io/answers/how-to-represent-strings-in-yaml) and [yaml-multiline.info](https://yaml-multiline.info/) for more info.

Not all template are dynamic. Static strings are also valid templates and just render to themselves:

```yaml
requests:
  list_fish:
    method: GET
    # This is a valid template
    url: "https://myfishes.fish/fish"
    # Numbers and booleans can also be templates!
    query:
      number_param: 3 # Parses as the template "3"
      bool_param: false # Parses as "false"
```

## Escape Sequences

In some scenarios you may want to use the `{{` sequence to represent those literal characters, rather than the start of a template key. To achieve this, you can escape the sequence with an underscore inside it, e.g. `{_{`. If you want the literal string `{_{`, then add an extra underscore: `{__{`.

| Template                | Parses as                  |
| ----------------------- | -------------------------- |
| `{_{this is raw text}}` | `["{{this is raw text}}"]` |
| `{_{{field1}}`          | `["{", field("field1")]`   |
| `{__{{field1}}`         | `["{__", field("field1")]` |
| `{_`                    | `["{_"]` (no escaping)     |

## Why?

Why does Slumber have its own template language? Why not use Jinja/Handlebars/Tera/Liquid/etc?

- Rust integration. Not all template languages have a Rust interface that enables the flexibility that Slumber needs.
- Support for lazy expressions. Some languages require all available values in template to be precomputed, which is incompatible with Slumber's dynamic data sources.
- Binary values. Most template languages focus on generating strings and don't support binary output values. Binary values are necessary for Slumber because not all HTTP requests are strings. For example, loading an image from a file and uploading it to a server involves non-textual template values.
- Simplicity. Most template languages are written for the purpose of building websites, which means generating HTML. This involves complex features such as conditionals and for loops. Slumber's needs are much more narrow. By simplifying the template language, it reduces the level of complexity available to users. This is a tradeoff: an easier learning curve, at the cost of less power.
