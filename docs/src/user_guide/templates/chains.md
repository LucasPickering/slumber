# Chains

Chains are Slumber's most powerful feature. They allow you to dynamically build requests based on other responses, shell commands, and more.

## Chains in Practice

The most common example of a chain is with a login request. You can define a recipe to log in to a service using username+password, then get the returned API token to authenticate subsequent requests. Of course, we don't want to store our credentials in Slumber file, so we can also use chains to fetch those. Let's see this in action:

```yaml
chains:
  username:
    source: !file
      path: ./username.txt
  password:
    source: !file
      path: ./password.txt
  auth_token:
    source: !request
      recipe: login
    selector: $.token

requests:
  # This returns a response like {"token": "abc123"}
  login: !request
    method: POST
    url: "https://myfishes.fish/login"
    body:
      !json {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}",
      }

  get_user: !request
    method: GET
    url: "https://myfishes.fish/current-user"
    authentication: !bearer "{{chains.auth_token}}"
```

> For more info on the `selector` field, see [Data Extraction via JSONPath](./selector.md)

### Automatically Executing the Upstream Request

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

For more detail about the various trigger variants, including the syntax of the `expire` variant, see [the API docs](../../api/request_collection/chain_source.md#chain-request-trigger).

## Chaining Chains

Chains on their own are powerful enough, but what makes them _really_ cool is that the arguments to a chain are templates in themselves, meaning you can use [nested templates](./index.md#nested-templates) to chain chains to other chains! Wait, what?

Let's say the login response doesn't return JSON, but instead the response looks like this:

```
token:abc123
```

Clearly this isn't a well-designed API, but sometimes that's all you get. You can use a nested chain with [cut](https://man7.org/linux/man-pages/man1/cut.1.html) to parse this:

```yaml
chains:
  username:
    source: !file
      path: ./username.txt
  password_encrypted:
    source: !file
      path: ./password.txt
  password:
    source: !command
      command:
  auth_token_raw:
    source: !request
      recipe: login
  auth_token:
    source: !command
      command: ["cut", "-d':'", "-f2"]
      stdin: "{{chains.auth_token_raw}}"

requests:
  login: !request
    method: POST
    url: "https://myfishes.fish/login"
    body:
      !json {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}",
      }

  get_user: !request
    method: GET
    url: "https://myfishes.fish/current-user"
    authentication: !bearer "{{chains.auth_token}}"
```

This means you can use external commands to perform any manipulation on data that you want.
