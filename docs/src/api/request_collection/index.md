# Request Collection

The request collection is the primary configuration for Slumber. It defines which requests can be made, and how to make them. When running a `slumber` instance, a single collection file is loaded. If you want to work with multiple collections at once, you'll have to run multiple instances of Slumber.

Collection files are designed to be sharable, meaning you can commit them to your Git repo. The most common pattern is to create one collection per API repo, and check it into the repo so other developers of the API can use the same collection. This makes it easy for any new developer or user to learn how to use an API.

## Format & Loading

A collection is defined as a [YAML](https://yaml.org/) file. When you run `slumber`, it will search the current directory _and its parents_ for the following default collection files, in order:

- `slumber.yml`
- `slumber.yaml`
- `.slumber.yml`
- `.slumber.yaml`

Whichever of those files is found _first_ will be used. For any given directory, if no collection file is found there, it will recursively go up the directory tree until we find a collection file or hit the root directory. If you want to use a different file for your collection (e.g. if you want to store multiple collections in the same directory), you can override the auto-search with the `--file` (or `-f`) command line argument. E.g.:

```sh
slumber -f my-collection.yml
```

## Fields

A request collection supports the following top-level fields:

| Field      | Type                                                    | Description                                                                                                        | Default |
| ---------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------- |
| `profiles` | [`mapping[string, Profile]`](./profile.md)              | Static template values                                                                                             | `{}`    |
| `requests` | [`mapping[string, RequestRecipe]`](./request_recipe.md) | Requests Slumber can send                                                                                          | `{}`    |
| `chains`   | [`mapping[string, Chain]`](./chain.md)                  | Complex template values                                                                                            | `{}`    |
| `.ignore`  | Any                                                     | Extra data to be ignored by Slumber (useful with [YAML anchors](https://yaml.org/spec/1.2.2/#anchors-and-aliases)) |         |

## Examples

```yaml
profiles:
  local:
    name: Local
    data:
      host: http://localhost:5000
      user_guid: abc123
  prd:
    name: Production
    data:
      host: https://httpbin.org
      user_guid: abc123

chains:
  username:
    source: !file
      path: ./username.txt
  password:
    source: !prompt
      message: Password
    sensitive: true
  auth_token:
    source: !request
      recipe: login
    selector: $.token

# Use YAML anchors for de-duplication (Anything under .ignore will not trigger an error for unknown fields)
.ignore:
  base: &base
    headers:
      Accept: application/json
      Content-Type: application/json

requests:
  login: !request
    <<: *base
    method: POST
    url: "{{host}}/anything/login"
    body:
      !json {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}",
      }

  # Folders can be used to keep your recipes organized
  users: !folder
    requests:
      get_user: !request
        <<: *base
        name: Get User
        method: GET
        url: "{{host}}/anything/current-user"
        authentication: !bearer "{{chains.auth_token}}"

      update_user: !request
        <<: *base
        name: Update User
        method: PUT
        url: "{{host}}/anything/current-user"
        authentication: !bearer "{{chains.auth_token}}"
        body: !json { "username": "Kenny" }
```
