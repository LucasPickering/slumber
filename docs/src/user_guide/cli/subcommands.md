# Subcommands

## `slumber collection`

Show the [request collection file](../../api/request_collection/index.md). You can open the file in your [configured editor](../tui/editor.md) with `slumber collection --edit`.

## `slumber config`

Show the [global configuration file](../../api/configuration/index.md). You can open the file in your [configured editor](../tui/editor.md) with `slumber config --edit`.

## `slumber db`

Access and modify the local Slumber database. **This has an optional subcommand that provides direct access to the collection or request history.** Without the subcommand, it just opens a shell into the SQLite file. By default this executes `sqlite3` and thus requires `sqlite3` to be installed.

Open a shell to the database:

```
slumber db
```

Run a single query and exit:

```
slumber db 'select 1'
```

[Read more about the database.](../database.md)

### `slumber db collection`

View and manipulate stored collection history/state. Slumber uses a local database to store all request/response history, as well as UI state and other persisted values. **As a user, you rarely have to worry about this.** The most common scenario in which you _do_ have to is if you've renamed a collection file and want to migrate the history to match the new path. [See here for how to migrate collection files](../database.md#migrating-collections).

See `slumber db collection --help` for more options.

### `slumber db request`

View and modify your Slumber request history. Slumber stores every request sent **from the TUI** in a local SQLite database (requests are **never** stored in a remote server). You can find the database file with `slumber db --path`.

#### `slumber db request list`

List requests in a table.

```sh
slumber db request list # List all requests for the current collection
slumber db request list --all # List all requests for all collections
slumber db request list login # List all requests for the "login" recipe
slumber db request list login -p dev # List all requests for "login" under the "dev" profile
```

#### `slumber db request get`

Show a single request/response from history.

```sh
slumber db request get login # Get the most recent request/response for "login"
slumber db request get 548ba3e7-3b96-4695-9856-236626ea0495 # Get a particular request/response by ID (IDs can be retrieved from the `list` subcommand)
```

#### `slumber db request delete`

Delete requests from history by ID.

```sh
slumber db request delete 548ba3e7-3b96-4695-9856-236626ea0495
# Delete multiple requests
slumber db request list login --id-only | xargs slumber db request delete
```

## `slumber generate`

Generate an HTTP request in an external format. Currently the only supported format is cURL.

**Overrides**

The `generate` subcommand supports overriding template values in the same that `slumber request` does. See the [`request` subcommand docs](#slumber-request) for more.

See `slumber generate --help` for more options.

**Examples**

Given this request collection:

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish

requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
    query:
      big: true
```

```sh
slumber generate curl --profile production list_fishes
slumber generate curl --profile production list_fishes -o host=http://localhost:8000
```

## `slumber import`

Generate a Slumber collection file based on an external format.

See `slumber import --help` for more options.

**Disclaimer**

Importers are **approximate**. They'll give the you skeleton of a collection file, but don't expect 100% equivalency. They save a lot of tedious work for you, but you'll generally still need to do some manual work on the collection file to get what you want.

**Formats**

Supported formats:

- Insomnia
- [OpenAPI v3.0](https://spec.openapis.org/oas/v3.0.3) and [OpenAPI v3.1](https://spec.openapis.org/oas/v3.1.1.html)
- [VSCode `.rest`](https://github.com/Huachao/vscode-restclient)
- [JetBrains `.http`](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html)

**Examples**

The general format is:

```sh
slumber import <format> <input> [output]
```

Possible inputs are:

- `-` for stdin
- Path to a local file
- URL to download via HTTP

For example, to import from an Insomnia collection `insomnia.json`:

```sh
slumber import insomnia insomnia.json slumber.yml
# Or, to read from stdin and print to stdout
slumber import insomnia - < insomnia.json
```

Or to import an OpenAPI spec from a server:

```sh
slumber import openapi https://petstore3.swagger.io/api/v3/openapi.json slumber.yml
```

Requested formats:

- [Postman](https://github.com/LucasPickering/slumber/issues/417)

If you'd like another format supported, please [open an issue](https://github.com/LucasPickering/slumber/issues/new).

## `slumber new`

Generate a new Slumber collection file. The new collection will have some example data predefined.

**Examples**

```sh
# Generate and use a new collection at the default path of slumber.yml
slumber new
slumber

# Generate and use a new collection at a custom path
slumber new my-collection.yml
slumber -f my-collection.yml
```

## `slumber request`

Send an HTTP request. There are many use cases to which the CLI is better suited than the TUI for sending requests, including:

- Sending a single one-off request
- Sending many requests in parallel
- Automating requests in a script
- Sharing requests with others

See `slumber request --help` for more options.

**Overrides**

You can manually override profile values using CLI arguments. This means the template renderer will use the override value in place of rendering the profile field. For example:

```sh
slumber request list_fishes --override host=https://dev.myfishes.fish
```

**Exit Code**

By default, the CLI returns exit code 1 if there is a fatal error, e.g. the request failed to build or a network error occurred. If an HTTP response was received and parsed, the process will exit with code 0, regardless of HTTP status.

If you want to set the exit code based on the HTTP response status, use the flag `--exit-code`.

| Code | Reason                                              |
| ---- | --------------------------------------------------- |
| 0    | HTTP response received                              |
| 1    | Fatal error                                         |
| 2    | HTTP response had status >=400 (with `--exit-code`) |

**Examples**

Given this request collection:

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish

requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
    query:
      big: true
```

```sh
slumber request --profile production list_fishes
slumber rq -p production list_fishes # rq is a shorter alias
slumber -f fishes.yml -p production list_fishes # Different collection file
```
