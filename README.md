# Slumber

[![Test CI](https://github.com/github/docs/actions/workflows/test.yml/badge.svg)](https://github.com/LucasPickering/slumber/actions)
[![crates.io](https://img.shields.io/crates/v/slumber.svg)](https://crates.io/crates/slumber)
[![Sponsor](https://img.shields.io/github/sponsors/LucasPickering?logo=github)](https://github.com/sponsors/LucasPickering)

- [Home Page](https://slumber.lucaspickering.me)
- [Installation](https://slumber.lucaspickering.me/artifacts/)
- [Docs](https://slumber.lucaspickering.me/book/)
- [Changelog](https://slumber.lucaspickering.me/changelog/)

![Slumber example](/static/demo.gif)

Slumber is a TUI (terminal user interface) HTTP client. Define, execute, and share configurable HTTP requests. Slumber is built on some basic principles:

- It will remain free to use forever
- You own your data: all configuration and data is stored locally and can be checked into version control
- It will never be [enshittified](https://en.wikipedia.org/wiki/Enshittification)

## Features

- Usable as a TUI or CLI
- Source-first configuration, for easy persistence and sharing
- [Import from external formats (e.g. Insomnia)](https://slumber.lucaspickering.me/book/user_guide/import.html)
- [Build requests dynamically from other requests, files, and shell commands](https://slumber.lucaspickering.me/book/user_guide/templates/index.html)
- [Browse response data using JSONPath selectors](https://slumber.lucaspickering.me/book/user_guide/tui/filter_query.html)
- Switch between different environments easily using [profiles](https://slumber.lucaspickering.me/book/api/request_collection/profile.html)
- And more!

## Examples

Slumber is based around **collections**. A collection is a group of request **recipes**, which are templates for the requests you want to run. A simple collection could be:

```yaml
# slumber.yml
requests:
  get: !request
    method: GET
    url: https://httpbin.org/get

  post: !request
    method: POST
    url: https://httpbin.org/post
    body: !json { "id": 3, "name": "Slumber" }
```

Create this file, then run the TUI with `slumber`.

For a more extensive example, see [the docs](https://slumber.lucaspickering.me/book/getting_started.html).

## Development

If you want to contribute to Slumber, see `CONTRIBUTING.md` for guidelines, development instructions, etc.
