# Examples

## Profiles

Let's start with a simple example. Let's say you're working on a fish-themed website, and you want to make requests both to your local stack and the deployed site. Templates, combined with [profiles](../profiles.md), allow you to easily switch between hosts:

> For the purposes of these examples, I've made up some theoretical endpoints and responses, following standard REST practice. This isn't a real API but it should get the point across.
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
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
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

## Chaining requests

One of Slumber's most powerful tools is the ability to chain requests together: send request 1, get some data from its response, then include that in request 2. Here's a series of examples showing how you can accomplish this.

### Load data from response

If you want to send a request that includes data derived from a previous response, you can use the [`response`](../../api/template_functions.md#response) function.

```yaml
requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
  post_fish_list:
    method: POST
    url: "{{ host }}/fishes"
    body: "{{ response('list_fish') }}"
```

`response` on its own is not very useful. Typically you want to extract some data from the response to include in your request. See the next example for this.

### Data extraction via JSONPath

[JSONPath](https://jsonpath.com/) is a simple query language for extracting data from JSON documents. Slumber has a [`jsonpath`](../../api/template_functions.md#jsonpath) function for this purpose.

In this example, we extract the first fish from `list_fish` to get additional details about it:

```yaml
requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
  get_fish:
    method: GET
    url: "{{ host }}/fishes/{{ response('fish_list') | jsonpath('$[0].id') }}"
```

The JSONPath query here is `$[0].id`, meaning it selects the `id` property from the first fish in the response.

### Dynamic select lists with `select`

Fetching the first fish is neat and all, but what if you want to select which fish to fetch? Enter the [`select`](../../api/template_functions.md#select) function! You can combine `select` with `jsonpath` to build dynamic selection lists:

```yaml
requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
  get_fish:
    method: GET
    url: "{{ host }}/fishes/{{ response('fish_list') | jsonpath('$[*].id', mode='array') | select() }}"
```

Notice that in this example, the JSONPath has changed from `$[0].id` to `$[*].id`, so it selects the `id` property from _every_ fish in the response, creating a list of IDs. Piping that to `select` will pop up a dialog with all the available fish IDs.

Also note the `mode='array'` argument to `jsonpath`. This tells `jsonpath` to always return an array of values, even if there is only one available. This is necessary because `select` must take in an array. See the `mode` argument of [`jsonpath`](../../api/template_functions.md#jsonpath) for more info.

### Triggering upstream requests

These examples are all neat and fancy, but they rely on you manually running `list_fish`. If you want your list of available fish you update, you'll have to kick it off manually. But we can do better! `response` takes an additional argument called `trigger`, enabling Slumber to automatically trigger the upstream request (in this case, `list_fish`) under certain conditions. The options for `trigger` are:

- `"never"`: The default behavior
- `"no_history"`: Trigger only if `list_fish` has never been run before
- `"always"`: Trigger `list_fish` every time we send `get_fish`
- Duration: Trigger `list_fish` if the last response is older than a specific time span

The first 3 options are pretty straight forward, so let's dig in the Duration option. Let's say we don't think fish will be added or removed _that_ often, so only trigger `list_fish` if it's more than a day old.

```yaml
requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
  get_fish:
    method: GET
    url: "{{ host }}/fishes/{{ response('fish_list', trigger='1d') | jsonpath('$[*].id', mode='array') | select() }}"
```

That's it! Just add `trigger='1d'` and Slumber handles the rest. See the docs for [`response`](../../api/template_functions.md#response) for more info on the trigger duration format.

## Deduplicating template expressions

As the previous examples have shown, template expressions can get pretty complicated. Slumber's template language doesn't support variables or assignment, so how can we break a template up into simpler pieces? This is especially useful when you want to use the same complicated template in multiple places. We can achieve this through [dynamic profile values](../profiles.md#dynamic-profile-values):

```yaml
profiles:
  local:
    host: http://localhost:5000
    fish_id: "{{ response('fish_list', trigger='1d') | jsonpath('$[*].id', mode='array') | select() }}"

requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
  get_fish:
    method: GET
    url: "{{ host }}/fishes/{{ fish_id }}"
  delete_fish:
    method: DELETE
    url: "{{ host }}/fishes/{{ fish_id }}"
```

Now we can easily use that template in multiple recipes. **But**, what if we have multiple profiles? We wouldn't want to copy-paste that template across every profile. Using [composition](../composition.md), we can define the template in one place and share it in every profile:

```yaml
.base_profile_data: &base_profile_data
  fish_id: "{{ response('fish_list', trigger='1d') | jsonpath('$[*].id', mode='array') | select() }}"

profiles:
  local:
    <<: *base_profile_data
    host: http://localhost:5000
  production:
    <<: *base_profile_data
    host: https://myfishes.fish

requests:
  list_fish:
    method: GET
    url: "{{ host }}/fishes"
  get_fish:
    method: GET
    url: "{{ host }}/fishes/{{ fish_id }}"
  delete_fish:
    method: DELETE
    url: "{{ host }}/fishes/{{ fish_id }}"
```
