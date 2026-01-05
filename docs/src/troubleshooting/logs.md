# Logs

Each Slumber session logs information and events to a log file. This can often be helpful in debugging bugs and other issues with the app. Each Slumber session (including each CLI invocation) logs to a different file. The logs are stored in a temporary directory, meaning they're cleaned up automatically by your OS.

## Finding the Log File

In the TUI, you can find the log path for the current session by opening the help dialog with `?`. It will be listed under the General section.

In the CLI, the log path will be printed if the command fails. If you want to force it to print the log path with `--print-log-path`.

Once you have the path to a log file, you can watch the logs with `tail -f <log file>`, or get the entire log contents with `cat <log file>`.

## Increasing Verbosity

In some scenarios, the default logging level is not verbose enough to debug issues. To increase the verbosity, use the `--log-level` argument:

```sh
slumber --log-level <level> ...
```

The available log levels are, in increasing verbosity:

- `off`
- `error`
- `warn`
- `info`
- `debug`
- `trace`

This argument applies to both the CLI and TUI. If omitted, the default is `off`, however logging cannot be set below `warn` for file output. That means stderr output is disabled by default, but file output is always _at least_ `warn`.
