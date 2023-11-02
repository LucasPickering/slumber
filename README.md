# Slumber

- [Home Page](https://slumber.lucaspickering.me)
- [Installation](https://slumber.lucaspickering.me/artifacts/)
- [Docs](https://slumber.lucaspickering.me/book/)
- [Changelog](https://slumber.lucaspickering.me/changelog/)

Slumber is a TUI (terminal user interface) HTTP client. Define, execute, and share configurable HTTP requests.

## Examples

Slumber is based around **collections**. A collection is a group of request **recipes**, which are templates for the requests you want to run. A simple collection could be:

```yaml
# slumber.yml
id: example
requests:
  - id: get
    method: GET
    url: https://httpbin.org/get
```

Create this file, then run the TUI with `slumber`.
