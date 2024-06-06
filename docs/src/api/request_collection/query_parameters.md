# Query Parameters

Query parameters are a component of a request URL. They provide additional information to the server about a request. In a request recipe, query parameters can be defined in one of two formats:

- Mapping of `key: value`
- List of strings, in the format `<key>=<value>`

The mapping format is typically more readable, but the list format allows you to define the same query parameter multiple times. In either format, **the key is treated as a plain string but the value is treated as a template**.

> Note: If you need to include a `=` in your parameter _name_, you'll need to use the mapping format. That means there is currently no support for multiple instances of a parameter with `=` in the name. This is very unlikely to be a restriction in the real world, but if you need support for this please [open an issue](https://github.com/LucasPickering/slumber/issues/new/choose).

## Examples

```yaml
recipes:
  get_fishes_mapping: !request
    method: GET
    url: "{{host}}/get"
    query:
      big: true
      color: red
      name: "{{name}}"

  get_fishes_list: !request
    method: GET
    url: "{{host}}/get"
    query:
      - big=true
      - color=red
      - color=blue
      - name={{name}}
```
