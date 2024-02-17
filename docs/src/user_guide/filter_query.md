# Data Filtering & Querying

Slumber supports querying data structures to transform or reduce response data.

There are two main use cases for querying:

- In [chained template values](../api/chain.md), to extract data
  - Provided via chain's `selector` argument
- In the TUI response body browser, to limit the response data shown

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
    source: !file ./creds.json
    selector: $.user
  password:
    source: !file ./creds.json
    selector: $.pw
  auth_token:
    source: !request login
    selector: $.token

# Use YAML anchors for de-duplication
base: &base
  headers:
    Accept: application/json
    Content-Type: application/json

requests:
  login:
    <<: *base
    method: POST
    url: "https://myfishes.fish/anything/login"
    body: |
      {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}"
      }

  get_user:
    <<: *base
    method: GET
    url: "https://myfishes.fish/anything/current-user"
    query:
      auth: "{{chains.auth_token}}"
```

While this example simple extracts inner fields, JSONPath can be used for much more powerful transformations. See the [JSONPath docs](https://www.ietf.org/archive/id/draft-goessner-dispatch-jsonpath-00.html) for more examples.

<!-- TODO add screenshot of in-TUI querying -->
