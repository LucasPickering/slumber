# Request Collection

The request collection is the primary configuration for Slumber. It defines which requests can be made, and how to make them. When running a `slumber` instance, a single collection file is loaded. If you want to work with multiple collections at once, you'll have to run multiple instances of Slumber.

Collection files are designed to be sharable, meaning you can commit them to your Git repo. The most common pattern is to create one collection per API repo, and check it into the repo so other developers of the API can use the same collection. This makes it easy for any new developer or user to learn how to use an API.

## Format & Loading

A collection is defined as a [YAML](https://yaml.org/) file. When you run `slumber`, it will search the current directory _and its parents_ for the following default collection files, in order:

- `slumber.yml`
- `slumber.yaml`
- `.slumber.yml`
- `.slumber.yaml`

Whichever of those files is found _first_ will be used. For any given directory, if no collection file is found there, it will recursively go up the directory tree until we find a collection file or hit the root directory. If you want to use a different file for your collection (e.g. if you want to store multiple collections in the same directory), you can override the auto-search with the `--file` (or `-f`) command line argument. You can also pass a directory to `--file` to have it search that directory instead of the current one. E.g.:

```sh
slumber --file my-collection.yml
slumber --file ../another-project/
```

## Fields

A request collection supports the following top-level fields:

| Field      | Type                                                    | Description               | Default |
| ---------- | ------------------------------------------------------- | ------------------------- | ------- |
| `profiles` | [`mapping[string, Profile]`](./profile.md)              | Static template values    | `{}`    |
| `requests` | [`mapping[string, RequestRecipe]`](./request_recipe.md) | Requests Slumber can send | `{}`    |

In addition to these fields, any top-level field beginning with `.` will be ignored. This can be combined with [YAML anchors](https://yaml.org/spec/1.2.2/#anchors-and-aliases) to define reusable components in your collection file.

## Examples

```yaml
# Use YAML anchors for de-duplication. Normally unknown fields in the
# collection trigger an error; the . prefix tells Slumber to ignore this field
.base_profile: &base_profile
  username: "{{ file('username.txt') }}"
  password: "{{ prompt(message='Password', sensitive=true) }}"
  auth_token: "{{ response('login') | jsonpath('$.token') }}"

profiles:
  local:
    name: Local
    data:
      <<: *base_profile
      host: http://localhost:5000
      user_guid: abc123
  prd:
    name: Production
    data:
      <<: *base_profile
      host: https://httpbin.org
      user_guid: abc123

.base_request: &base_request
  headers:
    Accept: application/json

requests:
  login:
    <<: *base_request
    method: POST
    url: "{{ host }}/anything/login"
    body:
      type: json
      data: { "username": "{{ username }}", "password": "{{ password }}" }

  # Folders can be used to keep your recipes organized
  users:
    requests:
      get_user:
        <<: *base_request
        name: Get User
        method: GET
        url: "{{ host }}/anything/current-user"
        authentication: !bearer "{{ auth_token }}"

      update_user:
        <<: *base_request
        name: Update User
        method: PUT
        url: "{{ host }}/anything/current-user"
        authentication: !bearer "{{ auth_token }}"
        body: !json { "username": "Kenny" }
```
