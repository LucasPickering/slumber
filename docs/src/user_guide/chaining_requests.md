# Chaining Requests

Sometimes you want to fetch a value with one request, then use the returned value in another request. For example, using a login request to fetch an authentication token, then using that to authenticate subsequent requests.

```yaml
chains:
  auth_token:
    source: !request
      recipe: login
    selector: $.token

base: &base
  headers:
    Accept: application/json
    Content-Type: application/json

recipes:
  login: !recipe
    <<: *base
    method: POST
    url: "https://myfishes.fish/login"
    body: |
      {
        "username": "username",
        "password": "password"
      }

  get_user: !recipe
    <<: *base
    method: GET
    url: "https://myfishes.fish/current-user"
    authentication: !bearer "{{chains.auth_token}}"
```

> For more info on the `selector` field, see [Data Filtering & Querying](./filter_query.md)

By default, the chained request (i.e. the "upstream" request) has to be executed manually to get the login token. You can have the upstream request automatically execute using the `trigger` field:

```yaml
chains:
  auth_token:
    source: !request
      recipe: login
      # Execute only if we've never logged in before
      trigger: !no_history
    selector: $.token
---
chains:
  auth_token:
    source: !request
      recipe: login
      # Execute only if the latest response is older than a day. Useful if your
      # token expires after a fixed amount of time
      trigger: !expire 1d
    selector: $.token
---
chains:
  auth_token:
    source: !request
      recipe: login
      # Always execute
      trigger: !always
    selector: $.token
```

For more detail about the various trigger variants, including the syntax of the `expire` variant, see [the API docs](../api/request_collection/chain_source.md#chain-request-trigger).
