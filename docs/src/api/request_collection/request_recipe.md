≈# Request Recipe

A request recipe defines how to make a particular request. For a REST API, you'll typically create one request recipe per endpoint. Other HTTP tools often call this just a "request", but that name can be confusing because "request" can also refer to a single instance of an HTTP request. Slumber uses the term "recipe" because it's used to render many requests. The word "template" would work as a synonym here, although we avoid that term here because it also refers to [string templates](../../user_guide/templates/index.md).

Recipes can be organized into folders. This means your set of recipes can form a tree structure. Folders are purely organizational, and don't impact the behavior of their child recipes at all.

**The IDs of your folders/recipes must be globally unique.** This means you can't have two recipes (or two folders, or one recipe and one folder) with the same associated key, even if they are in different folders. This restriction makes it easy to refer to recipes unambiguously using a single ID, which is helpful for CLI usage and data storage.

## Recipe Fields

The tag for a recipe is `!request` (see examples).

| Field            | Type                                                               | Description                                                                   | Default                |
| ---------------- | ------------------------------------------------------------------ | ----------------------------------------------------------------------------- | ---------------------- |
| `name`           | `string`                                                           | Descriptive name to use in the UI                                             | Value of key in parent |
| `method`         | `string`                                                           | HTTP request method                                                           | Required               |
| `url`            | [`Template`](../../user_guide/templates/index.md)                  | HTTP request URL                                                              | Required               |
| `query`          | [`mapping[string, QueryParameterValue]`](./query_parameters.md)    | URL query parameters                                                          | `{}`                   |
| `headers`        | [`mapping[string, Template]`](../../user_guide/templates/index.md) | HTTP request headers                                                          | `{}`                   |
| `authentication` | [`Authentication`](./authentication.md)                            | Authentication scheme                                                         | `null`                 |
| `body`           | [`RecipeBody`](./recipe_body.md)                                   | HTTP request body                                                             | `null`                 |
| `persist`        | `boolean`                                                          | Enable/disable request persistence. [Read more](../../user_guide/database.md) | `true`                 |

## Folder Fields

The tag for a folder is `!folder` (see examples).

| Field      | Type                                                    | Description                         | Default                |
| ---------- | ------------------------------------------------------- | ----------------------------------- | ---------------------- |
| `name`     | `string`                                                | Descriptive name to use in the UI   | Value of key in parent |
| `children` | [`mapping[string, RequestRecipe]`](./request_recipe.md) | Recipes organized under this folder | `{}`                   |

## Examples

```yaml
recipes:
  login: !request
    name: Login
    method: POST
    url: "{{host}}/anything/login"
    headers:
      accept: application/json
    query:
      root_access: yes_please
    body:
      !json {
        "username": "{{chains.username}}",
        "password": "{{chains.password}}",
      }
  fish: !folder
    name: Users
    requests:
      create_fish: !request
        method: POST
        url: "{{host}}/fishes"
        body: !json { "kind": "barracuda", "name": "Jimmy" }

      list_fish: !request
        method: GET
        url: "{{host}}/fishes"
        query:
          big: true
```
