# Chain Source

A chain source defines how a [Chain](./chain.md) gets its value. It populates the `source` field of a chain. There are multiple source types, and the type is specified using [YAML's tag syntax](https://yaml.org/spec/1.2.2/#24-tags).

## Examples

See the [`Chain`](./chain.md) docs for more complete examples.

```yaml
!request
recipe: login
trigger: !expire 12h
---
!command
command: ["echo", "-n", "hello"]
---
!env
variable: USERNAME
---
!file
path: ./username.txt
---
!prompt
message: Enter Password
```

## Variants

| Variant    | Type                                                | Description                                                     |
| ---------- | --------------------------------------------------- | --------------------------------------------------------------- |
| `!request` | [`ChainSource::Request`](#request)                  | Body of the most recent response for a specific request recipe. |
| `!command` | [`ChainSource::Command`](#command)                  | Stdout of the executed command                                  |
| `!env`     | [`ChainSource::Environment`](#environment-variable) | Value of an envionrment variable, or empty string if undefined  |
| `!file`    | [`ChainSource::File`](#file)                        | Contents of the file                                            |
| `!prompt`  | [`ChainSource::Prompt`](#prompt)                    | Value entered by the user                                       |
| `!select`  | [`ChainSource::Select`](#select)                    | User selects a value from a list                                |

### Request

Chain a value from the body of another response. This can reference either

| Field     | Type                                            | Description                                                                   | Default  |
| --------- | ----------------------------------------------- | ----------------------------------------------------------------------------- | -------- |
| `recipe`  | `string`                                        | Recipe to load value from                                                     | Required |
| `trigger` | [`ChainRequestTrigger`](#chain-request-trigger) | When the upstream recipe should be executed, as opposed to loaded from memory | `!never` |
| `section` | [`ChainRequestSection`](#chain-request-section) | The section (header or body) of the request from which to chain a value       | `Body`   |

#### Chain Request Trigger

This defines when a chained request should be triggered (i.e. when to execute a new request) versus when to use the most recent from history.

| Variant      | Type       | Description                                                                                                                |
| ------------ | ---------- | -------------------------------------------------------------------------------------------------------------------------- |
| `never`      | None       | Never trigger. The most recent response in history for the upstream recipe will always be used; error out if there is none |
| `no_history` | None       | Trigger only if there is no response in history for the upstream recipe                                                    |
| `expire`     | `Duration` | Trigger if the most recent response for the upstream recipe is older than some duration, or there is none                  |
| `always`     | None       | Always execute the upstream request                                                                                        |

`Duration` is specified as an integer followed by a unit (with no space). Supported units are:

- `s` (seconds)
- `m` (minutes)
- `h` (hours)
- `d` (days)

#### Examples

```yaml
!request
recipe: login
trigger: !never # This is the default, so the same as omitting
---
!request
recipe: login
trigger: !no_history
---
!request
recipe: login
trigger: !expire 12h
---
!request
recipe: login
trigger: !expire 30s
---
!request
recipe: login
trigger: !always
```

### Chain Request Section

This defines which section of the response (headers or body) should be used to load the value from.

| Variant  | Type       | Description                                                                                                                  |
| -------- | ---------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `body`   | None       | The body of the response                                                                                                     |
| `header` | `Template` | A specific header from the response. If the header appears multiple times in the response, only the first value will be used |

#### Examples

```yaml
!request
recipe: login
section: !header Token # This will take the value of the 'Token' header
```

### Command

Execute a command and use its stdout as the rendered value.

| Field     | Type         | Description                                                 | Default  |
| --------- | ------------ | ----------------------------------------------------------- | -------- |
| `command` | `Template[]` | Command to execute, in the format `[program, ...arguments]` | Required |
| `stdin`   | `Template`   | Standard input which will be piped into the command         | None     |

```yaml
username:
  source: !command
    command: [whoami]
```

### Environment Variable

Load a value from an environment variable.

| Field      | Type       | Description      | Default  |
| ---------- | ---------- | ---------------- | -------- |
| `variable` | `Template` | Variable to load | Required |

#### Examples

```yaml
current_dir:
  source: !env
    variable: PWD
```

### File

Read a file and use its contents as the rendered value.

| Field  | Type       | Description                                              | Default  |
| ------ | ---------- | -------------------------------------------------------- | -------- |
| `path` | `Template` | Path of the file to load (relative to current directory) | Required |

#### Examples

```yaml
username:
  source: !file
    path: ./username.txt
```

### Prompt

Prompt the user for text input to use as the rendered value.

| Field     | Type       | Description                                                                                                                                   | Default  |
| --------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| `message` | `Template` | Descriptive prompt for the user                                                                                                               | Chain ID |
| `default` | `Template` | Value to pre-populated the prompt textbox. **Note**: Due to a library limitation, not supported on chains with `sensitive: true` _in the CLI_ | `null`   |

#### Examples

```yaml
password:
  source: !prompt
    message: Enter Password
  sensitive: true
```

### Select

Prompt the user to select a defined value from a list.

| Field     | Type                               | Description                            | Default  |
| --------- | ---------------------------------- | -------------------------------------- | -------- |
| `message` | `Template`                         | Descriptive prompt for the user        | Chain ID |
| `options` | [`SelectOptions`](#Select-options) | List of options to present to the user | Required |

#### Select Options

The list of options to present to the user. This can be a static list of values or a dynamic configuration to generate the list of options.

| Variant   | Type                                              | Description                                                  |
| --------- | ------------------------------------------------- | ------------------------------------------------------------ |
| `fixed`   | `Template[]`                                      | A fixed list of options                                      |
| `dynamic` | [`DynamicSelectOptions`](#dynamic-select-options) | A dynamic configuration used to generate the list of options |

#### Examples

```yaml
fruit:
  souce: !select
    message: Select Fruit
    options:
      - apple
      - banana
      - guava
dynamic_fruit:
  source: !select
    message: Select Fruit
    options:
      # Assume this respones body looks like: {"fruits": ["apple", "guava", "pear"]}
      source: "{{chains.request_fruit}}"
      selector: $.fruits[*]
```

### Dynamic Select Options

This defines a dynamic configuration used to generate the list of options in a select chain. The `source` output could be any JSON value, in which case a `selector` must be used to filter to a JSON array.
If the `source` output is already a JSON array, no selector is required.

| Field      | Type                                                                                   | Description                                                                        | Default  |
| ---------- | -------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | -------- |
| `source`   | `Template`                                                                             | The source of the data to drive the list of options.                               | Required |
| `selector` | [`JSONPath`](https://www.ietf.org/archive/id/draft-goessner-dispatch-jsonpath-00.html) | A JSONPath expression to filter down to a JSON array.                              | `null`   |
