# Subcommands

<!-- toc -->

## `slumber collections`

View and manipulate stored collection history/state. Slumber uses a local database to store all request/response history, as well as UI state and other persisted values. **As a user, you rarely have to worry about this.** The most common scenario in which you _do_ have to is if you've renamed a collection file and want to migrate the history to match the new path. [See here for how to migrate collection files](../user_guide/database.md#migrating-collections).

See `slumber collections --help` for more options.

## `slumber db`

Access the local Slumber database file. This is an advanced command; most users never need to manually view or modify the database file. By default this executes `sqlite3` and thus requires `sqlite3` to be installed.

Open a shell to the database:

```
slumber db
```

Run a single query and exit:

```
slumber db 'select 1'
```

## `slumber generate`

Generate an HTTP request in an external format. Currently the only supported format is cURL.

### Overrides

The `generate` subcommand supports overriding template values in the same that `slumber request` does. See the [`request` subcommand docs](#overrides) for more.

See `slumber generate --help` for more options.

### Examples

Given this request collection:

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish

requests:
  list_fish: !request
    method: GET
    url: "{{host}}/fishes"
    query:
      - big=true
```

```sh
slumber generate curl --profile production list_fishes
slumber generate curl --profile production list_fishes -o host=http://localhost:8000
```

## `slumber history`

View and modify your Slumber request history. Slumber stores every command sent **from the TUI** in a local SQLite database (requests are **not** stored remotely). You can find the database file with `slumber show paths db`.

You can use the `slumber history` subcommand to browse and delete request history.

### `slumber history list`

List requests in a table.

```sh
slumber history list # List all requests for the current collection
slumber history list --all # List all requests for all collections
slumber history list login # List all requests for the "login" recipe
slumber history list login -p dev # List all requests for "login" under the "dev" profile
```

### `slumber history get`

Show a single request/response from history.

```sh
slumber history get login # Get the most recent request/response for "login"
slumber history get 548ba3e7-3b96-4695-9856-236626ea0495 # Get a particular request/response by ID (IDs can be retrieved from the `list` subcommand)
```

### `slumber history delete`

Delete requests from history by ID.

```sh
slumber history delete 548ba3e7-3b96-4695-9856-236626ea0495
# Delete multiple requests
slumber history list login --id-only | xargs slumber history delete
```

## `slumber import`

Generate a Slumber collection file based on an external format.

See `slumber import --help` for more options.

### Disclaimer

Importers are **approximate**. They'll give the you skeleton of a collection file, but don't expect 100% equivalency. They save a lot of tedious work for you, but you'll generally still need to do some manual work on the collection file to get what you want.

### Formats

Supported formats:

- Insomnia
- [OpenAPI v3.0](https://spec.openapis.org/oas/v3.0.3)
  - Note: Despite the minor version bump, OpenAPI v3.1 is _not_ backward compatible with v3.0. If you have a v3.1 spec, it _may_ work with this importer, but no promises.
- [VSCode `.rest`](https://github.com/Huachao/vscode-restclient)
- [JetBrains `.http`](https://www.jetbrains.com/help/idea/http-client-in-product-code-editor.html)

### Examples

The general format is:

```sh
slumber import <format> <path|url> [output]
```

For example, to import from an Insomnia collection `insomnia.json`:

```sh
slumber import insomnia insomnia.json slumber.yml
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

### Examples

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

### Overrides

You can manually override template values using CLI arguments. This means the template renderer will use the override value in place of calculating it. For example:

```sh
slumber request list_fishes --override host=https://dev.myfishes.fish
```

This can also be used to override chained values:

```sh
slumber request login --override chains.password=hunter2
```

### Exit Code

By default, the CLI returns exit code 1 if there is a fatal error, e.g. the request failed to build or a network error occurred. If an HTTP response was received and parsed, the process will exit with code 0, regardless of HTTP status.

If you want to set the exit code based on the HTTP response status, use the flag `--exit-code`.

| Code | Reason                                              |
| ---- | --------------------------------------------------- |
| 0    | HTTP response received                              |
| 1    | Fatal error                                         |
| 2    | HTTP response had status >=400 (with `--exit-code`) |

### Examples

Given this request collection:

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish

requests:
  list_fish: !request
    method: GET
    url: "{{host}}/fishes"
    query:
      - big=true
```

```sh
slumber request --profile production list_fishes
slumber rq -p production list_fishes # rq is a shorter alias
slumber -f fishes.yml -p production list_fishes # Different collection file
```

## `slumber show`

Print metadata about Slumber.

See `slumber show --help` for more options.

### Examples

```sh
slumber show paths # Show paths of various Slumber data files/directories
slumber show config # Print global configuration
slumber show config --edit # Edit global configuration
slumber show collection # Print collection file
slumber show collection --edit # Edit collection file
```
