# Templates

Templates enable dynamic string/binary construction. Slumber's template language is relatively simple, compared to complex HTML templating languages like Handlebars or Jinja. The goal is to be intuitive and unsurprising. It doesn't support complex features like loops, conditionals, etc.

Most string values in a request collection (e.g. URL, request body, etc.) are templates. Map keys (e.g. recipe ID, profile ID) are _not_ templates; they must be static strings.

The syntax for injecting a dynamic value into a template is double curly braces `{{...}}`. The contents inside the braces tell Slumber how to retrieve the dynamic value.

This guide serves as a functional example of how to use templates. For detailed information on options available, see [the API reference](../api/request_collection/template.md).

> **A note on YAML string syntax**
>
> One of the advantages (and disadvantages) of YAML is that it has a number of different string syntaxes. This enables you to customize your templates according to your specific needs around the behavior of whitespace and newlines. See [YAML's string syntaxes](https://www.educative.io/answers/how-to-represent-strings-in-yaml) and [yaml-multiline.info](https://yaml-multiline.info/) for more info on YAML strings.

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
      - big=true
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
    body: !json { "kind": "barracuda", "name": "Jimmy" }

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

## Binary Templates

While templates are mostly useful for generating strings, they can also generate binary data. This is most useful for sending binary request bodies. Some fields (e.g. URL) do _not_ support binary templates because they need valid text; in those cases, if the template renders to non-UTF-8 data, an error will be returned. In general, if binary data _can_ be supported, it is.

> Note: Support for binary form data is currently incomplete. You can render binary data from templates, but forms must be constructed manually. See [#235](https://github.com/LucasPickering/slumber/discussions/235) for more info.

```yaml
profiles:
  local:
    data:
      host: http://localhost:5000
      fish_id: "cod_father"

chains:
  fish_image:
    source: !file
      path: ./cod_father.jpg

requests:
  set_fish_image: !request
    method: POST
    url: "{{host}}/fishes/{{fish_id}}/image"
    headers:
      Content-Type: image/jpg
    body: "{{chains.fish_image}}"
```
