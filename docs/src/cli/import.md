# `slumber import`

Generate a Slumber collection file based on an external format.

See `slumber import --help` for more options.

## Disclaimer

Importers are **approximate**. They'll give the you skeleton of a collection file, but don't expect 100% equivalency. They save a lot of tedious work for you, but you'll generally still need to do some manual work on the collection file to get what you want.

## Examples

The general format is:

```sh
slumber import <format> <input> [output]
```

For example, to import from an Insomnia collection `insomnia.json`:

```sh
slumber import insomnia insomnia.json slumber.yml
```

## Formats

Supported formats:

- Insomnia
- [OpenAPI v3.0](https://spec.openapis.org/oas/v3.0.3)
  - Note: Despite the minor version bump, OpenAPI v3.1 is _not_ backward compatible with v3.0. If you have a v3.1 spec, it _may_ work with this importer, but no promises.
- [VSCode `.rest`](https://github.com/Huachao/vscode-restclient)
- [JetBrains `.http`](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html)

Requested formats:

If you'd like another format supported, please [open an issue](https://github.com/LucasPickering/slumber/issues/new).
