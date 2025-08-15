# Query Parameters

Query parameters are a component of a request URL. They provide additional information to the server about a request. In a request recipe, query parameters are defined as a map of `parameter: value`. The value can be a singular template (string/boolean/etc.) or a list of values.

```yaml
query:
  one: value
  many: [value1, value2]
```

A single query parameter can repeat multiple times in a URL; The above example will generate the query string `?one=value&many=value1&many=value2`.

> Note: Prior to version 4.0, Slumber supported a string-based query parameter format like `[one=value, many=value1, many=value2]`. This has been removed in 4.0 along with other breaking changes to the collection format. To migrate your collection file, see [v3 to v4 Migration](../../other/v4_migration.md).

## Examples

```yaml
recipes:
  get_fishes: !request
    method: GET
    url: "{{host}}/get"
    query:
      big: true
      color: [red, blue]
      name: "{{name}}"
```
