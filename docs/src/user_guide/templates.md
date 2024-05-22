# Templates

Templates enable dynamic string construction. Slumber's template language is relatively simple, compared to complex HTML templating languages like Handlebars or Jinja. The goal is to be intuitive and unsurprising. It doesn't support complex features like loops, conditionals, etc.

Most string _values_ (i.e. _not_ keys) in a request collection are templates, meaning they support templating. The syntax for templating a value into a string is double curly braces `{{...}}`. The contents inside the braces tell Slumber how to retrieve the dynamic value.

This guide serves as a functional example of how to use templates. For detailed information on options available, see [the API reference](../api/request_collection/template.md).

## A Basic Example

Let's start with a simple example. Let's say you're working on a fish-themed website, and you want to make requests both to your local stack and the deployed site. Templates, combined with profiles, allow you to easily switch between hosts:

> Note: for the purposes of these examples, I've made up some theoretical endpoints and responses, following standard REST practice. This isn't a real API but it should get the point across.
>
> Additionally, these examples will use the CLI because it's easy to demonstrate in text. All these concepts apply equally to the TUI.

```yaml
profiles:
  local:
    data:
      host: http://localhost:5000
  production:
    data:
      host: https://myfishes.fish

requests:
  list_fish: !request
    method: GET
    url: "{{host}}/fishes"
    query:
      big: true
```

Now you can easily select which host to hit. In the TUI, this is done via the Profile list. In the CLI, use the `--profile` option:

```sh
> slumber request --profile local list_fish
# http://localhost:5000/fishes
# Only one fish :(
[{"id": 1, "kind": "tuna", "name": "Bart"}]
> slumber request --profile production list_fish
# https://myfishes.fish/fishes
# More fish!
[
  {"id": 1, "kind": "marlin", "name": "Kim"},
  {"id": 2, "kind": "salmon", "name": "Francis"}
]
```

## Chaining Requests

Profile values are helpful when you want to switch between statically known values, but what if you need a value from a different response? Let's say you want to create a fish, then use its ID in a subsequent request. Then you want [**chains**](../api/request_collection/chain.md).

```yaml
profiles:
  local:
    data:
      host: http://localhost:5000

chains:
  fish_id:
    source: !request
      recipe: create_fish
    # This uses JSONPath to get a single value from the response body
    # https://jsonpath.com/
    selector: $.id

requests:
  create_fish: !request
    method: POST
    url: "{{host}}/fishes"
    body: >
      {"kind": "barracuda", "name": "Jimmy"}

  get_fish: !request
    method: GET
    url: "{{host}}/fishes/{{chains.fish_id}}"
```

Now we can make our requests back-to-back:

```sh
> slumber request -p local create_fish
# http://localhost:5000/fishes
{"id": 2, "kind": "barracuda", "name": "Jimmy"}
# http://localhost:5000/fishes/2
> slumber request -p local get_fish
{"id": 2, "kind": "barracuda", "name": "Jimmy"}
```

This demonstrates how to use chains to link responses to requests. Chains can link to other value sources though, including user-provided values (via a prompt) and shell commands. For a full list of chain types, see [the Chain API reference](../api/request_collection/chain.md).

## Nested Templates

What if you need a more complex chained value? Let's say the endpoint to get a fish requires the fish ID to be in the format `fish_{id}`. Why? Don't worry about it. Fish are particular. Templates support nesting implicitly. You can use this to compose template values into more complex strings. Just be careful not to trigger infinite recursion!

```yaml
profiles:
  local:
    data:
      host: http://localhost:5000
      fish_id: "fish_{{chains.fish_id}}"

chains:
  fish_id:
    source: !request
      recipe: create_fish
    selector: $.id

requests:
  create_fish: !request
    method: POST
    url: "{{host}}/fishes"
    body: >
      {"kind": "barracuda", "name": "Jimmy"}

  get_fish: !request
    method: GET
    url: "{{host}}/fishes/{{fish_id}}"
```

And let's see it in action:

```sh
> slumber request -p local create_fish
# http://localhost:5000/fishes
{"id": 2, "kind": "barracuda", "name": "Jimmy"}
> slumber request -p local get_fish
# http://localhost:5000/fishes/fish_2
{"id": "fish_2", "kind": "barracuda", "name": "Jimmy"}
```

## YAML String Syntax

One of the advantages (and disadvantages) of YAML is that it has a number of different string syntaxes. This enables you to customize your templates according to your specific needs around the behavior of whitespace and newlines. [yaml-multiline.info](https://yaml-multiline.info/) does a great job of demonstrating the differences.
