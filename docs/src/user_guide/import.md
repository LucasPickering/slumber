# Importing External Collections

Slumber can generate a collection file from external formats, making it easy to switch over. Currently the only supported format is Insomnia.

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

Requested formats:

- [OpenAPI](https://github.com/LucasPickering/slumber/issues/106)
- [JetBrains HTTP](https://github.com/LucasPickering/slumber/issues/122)

If you'd like another format supported, please [open an issue](https://github.com/LucasPickering/slumber/issues/new).
