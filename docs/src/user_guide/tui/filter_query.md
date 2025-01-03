# Data Filtering & Querying

When browsing an HTTP response in Slumber, you may want to filter, query, or otherwise transform the response to make it easier to view. Slumber supports this via embedded shell commands. The query box at the bottom of the response pane allows you to execute any shell command, which will be passed the response body via stdin and its output will be shown in the response pane. You can use `grep`, `jq`, `sed`, or any other text processing tool.

![Querying response via jq](../../images/query_jq.gif)

_Example of querying with jq_

![Querying response with pipes](../../images/query_pipe.gif)

_Example of using pipes in a query command_

## Side Effects

Keep in mind that your queries are being executed as shell commands on your system. You should avoid running any commands that interact with the file system, such as using `>` or `<` to pipe to/from files. TODO add more about side-effect commands once implemented

## Which shell does Slumber use?

By default, Slumber executes your command via `sh -c` on Unix and `cmd /S /C` on Windows. You can customize this via the [`commands.shell` configuration field](../../api/configuration/index.md#commandsshell). For example, to use `fish` instead of `sh`:

```yaml
commands:
  shell: [fish, -c]
```

If you don't want to execute via _any_ shell, you can set it to `[]`. In this case, query commands will be parsed via [shell-words](https://docs.rs/shell-words/latest/shell_words/) and executed directly. For example, `jq .args` will be parsed into `["jq", ".args"]`, then `jq` will be executed with a single argument: `.args`.
