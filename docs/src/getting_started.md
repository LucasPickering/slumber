# Getting Started

## Quick Start

Once you've [installed Slumber](/artifacts), setup is easy.

### 1. Create a Slumber collection file

Slumber's core feature is that it's **source-based**. That means you write down your configuration in a file first, then run Slumber and it reads the file. This differs from other popular clients such as Postman and Insomnia, where you define your configuration in the app, and it saves it to a file for you. The goal of being source-based is to make it easy to save and share your configurations.

The easiest way to get started is to generate a new collection with the `new` subcommand:

```sh
slumber new
```

### 2. Run Slumber

```sh
slumber
```

This will start the TUI, and you'll see the example requests available. Use tab/shift+tab (or the shortcut keys shown in the pane headers) to navigate around. Select a recipe in the left pane, then hit Enter to send a request.

## Going Further

Now that you have a collection, you'll want to customize it. Here's another example of a simple collection, showcasing multiple profiles:

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
      big: true
```

> Note: the `!request` tag, which tells Slumber that this is a request recipe, not a folder. This is [YAML's tag syntax](https://yaml.org/spec/1.2.2/#24-tags), which is used commonly throughout Slumber to provide explicit configuration.

This request collection uses [templates](./user_guide/templates/index.md) and [profiles](./api/request_collection/profile.md), allowing you to dynamically change the target host.

To learn more about the powerful features of Slumber you can use in your collections, keep reading with [Key Concepts](./user_guide/key_concepts.md).
