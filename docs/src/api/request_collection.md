# Request Collection

The request collection is the primary configuration for Slumber. It defines which requests can be made, and how to make them. When running a `slumber` instance, a single collection file is loaded. If you want to work with multiple collections at once, you'll have to run multiple instances of Slumber.

## Format & Loading

A collection is defined as a [YAML](https://yaml.org/) file. When you run `slumber`, it will search the current directory for the following default collection files, in order:

- `slumber.yml`
- `slumber.yaml`
- `.slumber.yml`
- `.slumber.yaml`

Whichever of those files is found _first_ will be used. If you want to use a different file for your collection (e.g. if you want to store multiple collections in the same directory), you can override the auto-search with the `--collection` (or `-c`) command line argument. E.g.:

```sh
slumber -c my-collection.yml
```

## Fields

A request collection supports the following top-level fields:

| Field      | Type                                         | Description               | Default |
| ---------- | -------------------------------------------- | ------------------------- | ------- |
| `profiles` | [`list[Profile]`](./profile.md)              | Static template values    | []      |
| `requests` | [`list[RequestRecipe]`](./request_recipe.md) | Requests Slumber can send | []      |
| `chains`   | [`list[Chain]`](./chain.md)                  | Complex template values   | []      |

## Examples

```yaml
profiles:
  - id: local
    name: Local
    data:
      host: http://localhost:5000
      user_guid: abc123
  - id: prd
    name: Production
    data:
      host: https://httpbin.org
      user_guid: abc123

chains:
  - id: username
    source: !file ./username.txt
  - id: password
    source: !prompt Password
    sensitive: true
  - id: auth_token
    source: !request login
    selector: $.token

# Use YAML anchors for de-duplication
base: &base
  headers:
    Accept: application/json
    Content-Type: application/json

requests:
  - <<: *base
    id: login
    method: POST
    url: "{{host}}/anything/login"
    body: |
      {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}"
      }

  - <<: *base
    id: Get User
    method: GET
    url: "{{host}}/anything/current-user"
    query:
      auth: "{{chains.auth_token}}"
```
