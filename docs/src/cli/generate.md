# `slumber generate`

Generate an HTTP request in an external format. Currently the only supported format is cURL.

## Examples

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
```

## Overrides

The `generate` subcommand supports overriding template values in the same that `slumber request` does. See the [`request` subcommand docs](./request.md#overrides) for more.

See `slumber generate --help` for more options.
