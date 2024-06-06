# `slumber request`

Send an HTTP request. There are many use cases to which the CLI is better suited than the TUI for sending requests, including:

- Sending a single one-off request
- Sending many requests in parallel
- Automating requests in a script
- Sharing requests with others

See `slumber request --help` for more options.

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
slumber request --profile production list_fishes
slumber rq -p production list_fishes # rq is a shorter alias
slumber -f fishes.yml -p production list_fishes # Different collection file
```

## Overrides

You can manually override template values using CLI arguments. This means the template renderer will use the override value in place of calculating it. For example:

```sh
slumber request list_fishes --override host=https://dev.myfishes.fish
```

This can also be used to override chained values:

```sh
slumber request login --override chains.password=hunter2
```

## Exit Code

By default, the CLI returns exit code 1 if there is a fatal error, e.g. the request failed to build or a network error occurred. If an HTTP response was received and parsed, the process will exit with code 0, regardless of HTTP status.

If you want to set the exit code based on the HTTP response status, use the flag `--exit-code`.

| Code | Reason                                              |
| ---- | --------------------------------------------------- |
| 0    | HTTP response received                              |
| 1    | Fatal error                                         |
| 2    | HTTP response had status >=400 (with `--exit-code`) |
