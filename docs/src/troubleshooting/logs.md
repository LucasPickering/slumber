# Logs

Each Slumber session logs information and events to a log file. This can often be helpful in debugging bugs and other issues with the app. All sessions of Slumber log to the same file. Currently there is no easy to way disambiguate between logs from parallel sessions :(

## Finding the Log File

To find the path to the log file, hit the `?` to open the help dialog. It will be listed under the General section. Alternatively, run the command `slumber show paths log`.

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
