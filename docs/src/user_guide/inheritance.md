# Collection Reuse & Inheritance

## The Problem

Let's start with an example of something that sucks. Let's say you're making requests to a fish-themed JSON API, and it requires authentication. Gotta protect your fish! Your request collection might look like so:

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish
      fish_id: 6

chains:
  token:
    source: !file
      path: ./api_token.txt

requests:
  list_fish: !request
    method: GET
    url: "{{host}}/fishes"
    query:
      big: true
    headers:
      Accept: application/json
    authentication: !bearer "{{chains.token}}"

  get_fish: !request
    method: GET
    url: "{{host}}/fishes/{{fish_id}}"
    headers:
      Accept: application/json
    authentication: !bearer "{{chains.token}}"
```

## The Solution

You've heard of [DRY](https://en.wikipedia.org/wiki/Don%27t_repeat_yourself), so you know this is bad. Every new request recipe requires re-specifying the headers, and if anything about the authorization changes, you have to change it in multiple places.

You can easily re-use components of your collection using [YAML's merge feature](https://yaml.org/type/merge.html).

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish

chains:
  token:
    source: !file
      path: ./api_token.txt

# The name here is arbitrary, pick any name you like
request_base: &request_base
  headers:
    Accept: application/json
  authentication: !bearer "{{chains.token}}"

requests:
  list_fish: !request
    <<: *request_base
    method: GET
    url: "{{host}}/fishes"
    query:
      big: true

  get_fish: !request
    <<: *request_base
    method: GET
    url: "{{host}}/fishes/{{chains.fish_id}}"
```

Great! That's so much cleaner. Now each recipe can inherit whatever base properties you want just by including `<<: *request_base`. This is still a bit repetitive, but it has the advantage of being explicit. You may have some requests that _don't_ want to include those values.

## Recursive Inheritance

But wait! What if you have a new request that needs an additional header? Unfortunately, YAML's merge feature does not support recursive merging. If you need to extend the `headers` map from the base request, you'll need to pull that map in manually:

```yaml
profiles:
  production:
    data:
      host: https://myfishes.fish

chains:
  token:
    source: !file
      path: ./api_token.txt

# The name here is arbitrary, pick any name you like
request_base: &request_base
  headers: &headers_base # This will let us pull in the header map to extend it
    Accept: application/json
  authentication: !bearer "{{chains.token}}"

requests:
  list_fish: !request
    <<: *request_base
    method: GET
    url: "{{host}}/fishes"
    query:
      big: true

  get_fish: !request
    <<: *request_base
    method: GET
    url: "{{host}}/fishes/{{chains.fish_id}}"

  create_fish: !request
    <<: *request_base
    method: POST
    url: "{{host}}/fishes"
    headers:
      <<: *headers_base
      Content-Type: application/json
    body: >
      {"kind": "barracuda", "name": "Jimmy"}
```
