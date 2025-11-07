# Command Line Interface

While Slumber is primary intended as a TUI, it also provides a Command Line Interface (CLI). The CLI can be used to send requests, just like the TUI. It also provides some utility commands for functionality not available in the TUI. For a full list of available commands see the side bar or run:

```sh
slumber help
```

Some common CLI use cases:

- [Send requests](./subcommands.md#slumber-request)
- [Import from an external format](./subcommands.md#slumber-import)
- [Generate request in an external format (e.g. curl)](./subcommands.md#slumber-generate)
- [View & edit Slumber configuration](./subcommands.md#slumber-config)

## Examples

The Slumber CLI can be composed with other CLI tools, making it a powerful tool for scripting and bulk tasks. Here are some examples of how to use it with common tools.

> Note: These examples are written for a POSIX shell (bash, zsh, etc.). It assumes some basic familiarity with shell features such as pipes. Unfortunately I have no shell experience with Windows so I can't help you there :(

### Filtering responses with `jq`

Let's say you want to fetch the name of each fish from your fish-tracking service. Here's your collection file:

```yaml
requests:
  list_fish:
    method: GET
    url: "https://myfishes.fish/fishes"
```

This endpoint returns a response like:

```json
[
  {
    "kind": "barracuda",
    "name": "Jimmy"
  },
  {
    "kind": "striped bass",
    "name": "Balthazar"
  },
  {
    "kind": "rockfish",
    "name": "Maureen"
  }
]
```

You can fetch this response and filter it down to just the names:

```sh
slumber rq list_fish | jq -r '.[].name'
```

And the output:

```
Jimmy
Balthazar
Maureen
```

### Running requests in parallel with `xargs`

Building on [the previous example](#filtering-responses-with-jq), let's say you want to fetch details on each fish returned from the list response. We'll add a `get_fish` recipe to the collection. By default, the fish name will come from a prompt:

```yaml
profiles:
  prd:
    fish_name: "{{ prompt(message='Which fish?') }}"

requests:
  list_fish:
    method: GET
    url: "https://myfishes.fish/fishes"

  get_fish:
    method: GET
    url: "https://myfishes.fish/fishes/{{ fish_name }}"
```

We can use `xargs` and the `-o` flag of `slumber request` to fetch details for each fish in parallel:

```sh
slumber rq list_fish | jq -r '.[].name' > fish.txt
cat fish.txt | xargs -L1 -I'{}' -P3 slumber rq get_fish --override fish_name={} --output {}.json
```

Let's break this down:

- `-L1` means to consume one argument (in this case, one fish name) per invocation of `slumber`
- `-I{}` sets the substitution string, i.e. the string that will be replaced with each argument
- `-P3` tells `xargs` the maximum number of processes to run concurrently, which in this case means the maximum number of concurrent requests
- Everything else is the `slumber` command
  - `--override fish_name={}`: `xargs` replaces `fish_name` with the argument from the file, so it doesn't prompt for a name
  - `--output {}.json` writes to a JSON file with the fish's name (e.g. `Jimmy.json`)
