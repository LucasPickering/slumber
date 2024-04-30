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

### Insomnia
For example, to import from an Insomnia collection `insomnia.json`:

```sh
slumber import insomnia insomnia.json slumber.yml
```

### Jetbrains HTTP

To import a Jetbrains HTTP file use the following command:
```sh
slumber import jetbrains jetbrains.http slumber.yml
```

If you would like to include your `http-client.env.json` into the slumber collection use:
```sh
slumber import jetbrains-with-public-env jetbrains.http slumber.yml
```
This searches for your `http-client.env.json` in the same directory.

If you would like to include your `http-client.env.json` and your `http-client.private.env.json` into the slumber collection use:
```sh
slumber import jetbrains-with-private-env jetbrains.http slumber.yml
```
This searches for both your `http-client.env.json` and `http-client.private.env.json` in the same directory.

Some advanced Jetbrains features are not included. 

#### Supported Jetbrains Features:
- HTTP requests
- Named requests 
- Inline variables 
- `http-client.env.json` variables

#### Unsupported Jetbrains Features:
- Running Javascript mid request
- Dynamic variables from a `.env` file
- Dynamic UUIDs and other fake data
- Piping the response into a Javascript file
- Websocket requests and other non HTTP schemes

### VSCode Rest Client  

[VSCode Rest Client](https://marketplace.visualstudio.com/items?itemName=humao.rest-client) is a popular VSCode extension.
This extension has been ported to [neovim](https://github.com/rest-nvim/rest.nvim), so as long as your file ends with `.rest` you can import it using this command:
```sh
slumber import vscode vscode.rest slumber.yml
```

#### Supported VSCode Rest Features:
- HTTP Requests
- Inline variables 

#### Unsupported VSCode Features:
- Dynamic variables from a `.env` file
- Dynamic UUIDs and other fake data


## Formats

Supported formats:

- Insomnia
- [VSCode REST Client](https://marketplace.visualstudio.com/items?itemName=humao.rest-client) 
- [Jetbrains HTTP files](https://www.jetbrains.com/help/idea/exploring-http-syntax.html)


Requested formats:

- [OpenAPI](https://github.com/LucasPickering/slumber/issues/106)

If you'd like another format supported, please [open an issue](https://github.com/LucasPickering/slumber/issues/new).
