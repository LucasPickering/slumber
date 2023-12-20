# Command Line Interface

While Slumber is primary intended as a TUI, it also provides a Command Line Interface (CLI). The CLI can be used to send requests, just like the TUI. It also provides some utility commands for functionality not available in the TUI. For a full list of available commands, run:

```sh
slumber help
```

## Sending Requests

There are many use cases to which the CLI is better suited than the TUI, including:

- Sending a single one-off request
- Sending many requests in parallel
- Automating requests in a script

Given this request collection:

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish

requests:
  list_fish:
    method: GET
    url: "{{host}}/fishes"
```

You can use the `request` subcommand:

```sh
slumber request --profile production list_fishes
slumber rq -p production list_fishes # This is equivalent, just shorter
```
