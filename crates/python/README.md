# slumber-python

> This is not related to, or a replacement of, the [slumber](https://pypi.org/project/slumber/) package.

[**Documentation**](https://slumber.lucaspickering.me/integration/python.html)

Python bindings for [Slumber](https://slumber.lucaspickering.me/), the source-based REST API client. This library makes it easy to take your existing Slumber collection and use it in Python scripts.

This package does not yet support all the same functionality as the [Slumber CLI](https://slumber.lucaspickering.me/user_guide/cli/index.html). If you have a specific feature that you'd like to see in it, please [open an issue on GitHub](https://github.com/LucasPickering/slumber/issues/new/choose).

**This is not a general-purpose REST/HTTP client.** If you're not already using Slumber as a TUI/CLI client, then there isn't much value provided by this package.

## Installation

```sh
pip install slumber-python
```

## Usage

First, [create a Slumber collection](https://slumber.lucaspickering.me/getting_started.html).

```py

from slumber import Collection

collection = Collection()
response = collection.request('example_get')
print(response.text)
```

For more usage examples, [see the docs](https://slumber.lucaspickering.me/integration/python.html).

## Versioning

For simplicity, the version of this package is synched to the main Slumber version and follows the same [releases](https://github.com/LucasPickering/slumber/releases). That means there may be releases of this package that don't actually change anything, or version bumps that are higher than necessary (e.g. minor versions with only patch changes). The versioning **is still semver compliant**.
