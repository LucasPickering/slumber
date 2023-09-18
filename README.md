# Slumber

Slumber is a TUI (terminal user interface) REST client. Define, execute, and share configurable HTTP requests.

## Example

Slumber is based around **collections**. A collection is a group of request **recipes**, which are templates for the requests you want to run. A simple collection could be:

```yaml
# slumber.yml
requests:
  - id: get
    method: GET
    url: https://httpbin.org/get
```

Create this file, then run the TUI with `slumber`.
