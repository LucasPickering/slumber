# Terminal User Interface

The Terminal User Interface (TUI) is the primary use case for Slumber. It provides a long-lived, interactive interface for sending HTTP requests, akin to Insomnia or Postman. The difference of course is Slumber runs entirely in the terminal.

To start the TUI, simply run:

```sh
slumber
```

This will detect your request collection file [according to the protocol](../api/request_collection/index.md#format--loading). If you want to load a different file, you can use the `--collection` parameter:

```sh
slumber --collection my-slumber.yml
```

## Auto-Reload

Once you start your Slumber, that session is tied to a single collection file. Whenever that file is modified, Slumber will automatically reload it and changes will immediately be reflected in the TUI. If auto-reload isn't working for some reason, you can manually reload the file with the `r` key.

## Multiple Sessions

Slumber supports running multiple sessions at once, even on the same collection. Request history is stored in a thread-safe [SQLite](https://www.sqlite.org/index.html), so multiple sessions can safely interact simultaneously.

If you frequently run multiple sessions together and want to quickly switch between them, consider a configurable terminal manager like [tmux](https://github.com/tmux/tmux/wiki) or [Zellij](https://zellij.dev/).
