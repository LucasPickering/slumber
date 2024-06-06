# Getting Started

## Quick Start

Once you've [installed Slumber](/artifacts), setup is easy.

### 1. Create a Slumber collection file

Slumber's core feature is that it's **source-based**. That means you write down your configuration in a file first, then run Slumber and it reads the file. This differs from other popular clients such as Postman and Insomnia. The goal of being source-based is to make it easy to save and share your configurations.

To get started, create a file called `slumber.yml` and add the following contents:

```yaml
requests:
  get: !request
    method: GET
    url: https://httpbin.org/get
```

> Note: the `!request` tag, which tells Slumber that this is a request recipe, not a folder. This is [YAML's tag syntax](https://yaml.org/spec/1.2.2/#24-tags), which is used commonly throughout Slumber to provide explicit configuration.

### 2. Run Slumber

```sh
slumber
```

## Going Further

Here's a more complete example:

```yaml
# slumber.yml
profiles:
  local:
    data:
      host: http://localhost:5000
  production:
    data:
      host: https://myfishes.fish

requests:
  create_fish: !request
    method: POST
    url: "{{host}}/fishes"
    body: !json { "kind": "barracuda", "name": "Jimmy" }

  list_fish: !request
    method: GET
    url: "{{host}}/fishes"
    query:
      - big=true
```

This request collection uses [templates](./user_guide//templates.md) and [profiles](./api/request_collection/profile.md) allow you to dynamically change the target host.
