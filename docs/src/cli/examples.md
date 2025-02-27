# Examples

The Slumber CLI can be composed with other CLI tools, making it a powerful tool for scripting and bulk tasks. Here are some examples of how to use it with common tools.

> Note: These examples are written for a POSIX shell (bash, zsh, etc.). It assumes some basic familiarity with shell features such as pipes. Unfortunately I have no shell experience with Windows so I can't help you there :(

## Filtering responses with `jq`

Let's say you want to fetch the name of each fish from your fish-tracking service. Here's your collection file:

```yaml
requests:
  list_fish: !request
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

## Running requests in parallel with `xargs`

Building on [the previous example](#filtering-responses-with-jq), let's say you want to fetch details on each fish returned from the list response. We'll add a `get_fish` recipe to the collection. By default, the fish name will come from a prompt:

```yaml
chains:
  fish_name:
    source: !prompt
      message: "Which fish?"

requests:
  list_fish: !request
    method: GET
    url: "https://myfishes.fish/fishes"

  get_fish: !request
    method: GET
    url: "https://myfishes.fish/fishes/{{chains.fish_name}}"
```

We can use `xargs` and the `-o` flag of `slumber request` to fetch details for each fish in parallel:

```sh
slumber rq list_fish | jq -r '.[].name' > fish.txt
cat fish.txt | xargs -L1 -I'{}' -P3 slumber rq get_fish -o chains.fish_name={} --output {}.json
```

Let's break this down:

- `-L1` means to consume one argument (in this case, one fish name) per invocation of `slumber`
- `-I{}` sets the substitution string, i.e. the string that will be replaced with each argument
- `-P3` tells `xargs` the maximum number of processes to run concurrently, which in this case means the maximum number of concurrent requests
- Everything else is the `slumber` command
  - `-o chains.fish_name={} `chains.fish_name` with the argument from the file, so it doesn't prompt for a name
  - `--output {}.json` writes to a JSON file with the fish's name (e.g. `Jimmy.json`)
