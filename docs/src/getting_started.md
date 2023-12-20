# Getting Started

## Quick Start

Once you've [installed Slumber](/artifacts), setup is easy.

### 1. Create a Slumber collection file

Create a file called `slumber.yml` and add the following contents:

```yaml
requests:
  get:
    method: GET
    url: https://httpbin.org/get
```

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
  create_fish:
    method: POST
    url: "{{host}}/fishes"
    body: >
      {"kind": "barracuda", "name": "Jimmy"}

  list_fish:
    method: GET
    url: "{{host}}/fishes"
```

This request collection uses [templates](./user_guide//templates.md) and [profiles](./api/profile.md) allow you to dynamically change the target host.
