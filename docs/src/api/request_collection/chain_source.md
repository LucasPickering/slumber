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
!file
path: ./username.txt
---
!prompt
message: Enter Password
```

## Variants

| Variant   | Type                               | Description                                                     |
| --------- | ---------------------------------- | --------------------------------------------------------------- |
| `request` | [`ChainSource::Request`](#request) | Body of the most recent response for a specific request recipe. |
| `command` | [`ChainSource::Command`](#command) | Stdout of the executed command                                  |
| `file`    | [`ChainSource::File`](#file)       | Contents of the file                                            |
| `prompt`  | [`ChainSource::Prompt`](#prompt)   | Value entered by the user                                       |

### Request

Chain a value from the body of another response. This can reference either

| Field     | Type                                            | Description                                                                   | Default  |
| --------- | ----------------------------------------------- | ----------------------------------------------------------------------------- | -------- |
| `recipe`  | `string`                                        | Recipe to load value from                                                     | Required |
| `trigger` | [`ChainRequestTrigger`](#chain-request-trigger) | When the upstream recipe should be executed, as opposed to loaded from memory | `!never` |

### Chain Request Trigger

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

### Command

Execute a command and use its stdout as the rendered value.

| Field     | Type       | Description                                                 | Default  |
| --------- | ---------- | ----------------------------------------------------------- | -------- |
| `command` | `string[]` | Command to execute, in the format `[program, ...arguments]` | Required |

### File

Read a file and use its contents as the rendered value.

| Field  | Type     | Description                                              | Default  |
| ------ | -------- | -------------------------------------------------------- | -------- |
| `path` | `string` | Path of the file to load (relative to current directory) | Required |

### Prompt

Prompt the user for input to use as the rendered value.

| Field     | Type     | Description                     | Default  |
| --------- | -------- | ------------------------------- | -------- |
| `message` | `string` | Descriptive prompt for the user | Required |
