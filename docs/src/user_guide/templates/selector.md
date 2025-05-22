# Data Extraction via JSONPath

[Chains](./chains.md) support querying data structures to transform or reduce response data. THis is done via the `selector` field of a chain.

**Regardless of data format, querying is done via [JSONPath](https://www.ietf.org/archive/id/draft-goessner-dispatch-jsonpath-00.html).** For non-JSON formats, the data will be converted to JSON, queried, and converted back. This keeps querying simple and uniform across data types.

## Querying Chained Values

Here's some examples of using queries to extract data from a chained value. Let's say you have two chained value sources. The first is a JSON file, called `creds.json`. It has the following contents:

```json
{ "user": "fishman", "pw": "hunter2" }
```

We'll use these credentials to log in and get an API token, so the second data source is the login response, which looks like so:

```json
{ "token": "abcdef123" }
```

```yaml
chains:
  username:
    # Slumber knows how to query this file based on its extension
    source: !file
      path: ./creds.json
    selector: $.user
  password:
    source: !file
      path: ./creds.json
    selector: $.pw
  auth_token:
    source: !request
      recipe: login
    selector: $.token

requests:
  login: !request
    method: POST
    url: "https://myfishes.fish/anything/login"
    body:
      !json {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}",
      }

  get_user: !request
    method: GET
    url: "https://myfishes.fish/anything/current-user"
    query:
      auth: "{{chains.auth_token}}"
```

While this example simple extracts inner fields, JSONPath can be used for much more powerful transformations. See the [JSONPath docs](https://www.ietf.org/archive/id/draft-goessner-dispatch-jsonpath-00.html) or [this JSONPath editor](https://jsonpath.com/) for more examples.

### More Powerful Querying with Nested Chains

If JSONPath isn't enough for the data extraction you need, you can use nested chains to filter with whatever external programs you want. For example, if you want to use `jq` instead:

```yaml
chains:
  username:
    source: !file
      path: ./creds.json
    selector: $.user
  password:
    source: !file
      path: ./creds.json
    selector: $.pw
  auth_token_raw:
    source: !request
      recipe: login
  auth_token:
    source: !command
      command: [ "jq", ".token" ]
      stdin: "{{chains.auth_token_raw}}

requests:
  login: !request
    method: POST
    url: "https://myfishes.fish/anything/login"
    body: !json
      {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}"
      }

  get_user: !request
    method: GET
    url: "https://myfishes.fish/anything/current-user"
    query:
      auth: "{{chains.auth_token}}"
```

You can use this capability to manipulate responses via `grep`, `awk`, or any other program you like.
