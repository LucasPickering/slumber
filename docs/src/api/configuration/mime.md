# MIME Maps

Some configuration fields support a mapping of [MIME types](https://en.wikipedia.org/wiki/Media_type) (AKA media types or content types). This allow you to set multiple values for the configuration field, and the correct value will be selected based on the MIME type of the relevant recipe/request/response.

The keys of this map are glob-formatted (i.e. wildcard) MIME types. For example, if you're configuring your pager and you want to use `hexdump` for all images, `fx` for JSON, and `less` for everything else:

```yaml
pager:
  image/*: hexdump
  application/json: fx
  "*/*": less
```

> **Note:** Paths are matched top to bottom, so `*/*` **should always go last**. Any pattern starting with `*` must be wrapped in quotes in order to be parsed as a string.

- `image/png`: matches `image/*`
- `image/jpeg`: matches `image/*`
- `application/json`: matches `application/json`
- `text/csv`: matches `*/*`

## Aliases

In addition to accepting MIME patterns, there are also predefined aliases to make common matches more convenient:

| Alias     | Maps To             |
| --------- | ------------------- |
| `default` | `*/*`               |
| `json`    | `application/*json` |
| `image`   | `image/*`           |

## Notes on Matching

- Matching is done top to bottom, and **the first matching pattern will be used**. For this reason, your `*/*` pattern **should always be last**.
- Matching is performed just against the [essence string](https://docs.rs/mime/latest/mime/struct.Mime.html#method.essence_str) of the recipe/request/response's `Content-Type` header, i.e. the `type/subtype` only. In the example `multipart/form-data; boundary=ABCDEFG`, the semicolon and everything after it **is not included in the match**.
- Matching is performed by the [Rust glob crate](https://docs.rs/glob/latest/glob/struct.Pattern.html). Despite being intended for matching file paths, it works well for MIME types too because they are also `/`-delimited
