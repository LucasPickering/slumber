# Logs

Each Slumber session logs information and events to a log file. This can often be helpful in debugging bugs and other issues with the app. Each session of Slumber logs to its own file, and files are stored in a temporary directory so they are automatically cleaned up periodically by the operating system.

## Finding the Log File

To find the path to a particular TUI session's log file, hit the `?` to open the help dialog. It will be listed under the General section. To find logs for a CLI command, TODO.

> Note: Each Slumber session, including each invocation of the CLI, will log to its own file. That means there's no easy way to retroactively find the logs for a CLI command if you didn't pass TODO. Instead, you can find the log directory with `slumber show paths`, then search each log file in that directory to find the one associated with the command in question. If the thing you're debugging is reproducible though, it's easier just to run it again with TODO.

Once you have the path to a log file, you can watch the logs with `tail -f <log file>`, or get the entire log contents with `cat <log file>`.

## Increasing Verbosity

In some scenarios, the default logging level is not verbose enough to debug issues. To increase the verbosity, set the `RUST_LOG` environment variable when starting Slumber:

```sh
RUST_LOG=slumber=<level> slumber ...
```

The `slumber=` filter applies this level only to Slumber's internal logging, instead of all libraries, to cut down on verbosity that will likely not be helpful. The available log levels are, in increasing verbosity:

- `error`
- `warn`
- `info`
- `debug`
- `trace`
