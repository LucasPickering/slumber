# In-App Editing

Slumber supports editing your collection file without leaving the app. To do so, open the actions menu (`x` by default), then select `Edit Collection`. Slumber will open an external editor to modify the file. To determine which editor to use, Slumber checks these places in the following order:

- `editor` field of the [configuration file](./index.md)
- `VISUAL` environment variable
- `EDITOR` environment variable
- Default to `vim`

The `VISUAL` and `EDITOR` environment variables are a common standard to define a user's preferred text editor. For example, it's what [git uses by default](https://git-scm.com/book/en/v2/Customizing-Git-Git-Configuration) to determine how to edit commit messages. If you want to use the same editor for all programs, you should set these. If you want to use a command specific to Slumber, set the `editor` config field.

Slumber supports passing additional arguments to the editor. For example, if you want to open `VSCode` and have wait for the file to be saved, you can configure your editor like so:

```yaml
editor: code --wait
```

The command will be parsed like a shell command (although a shell is never actually invoked). For exact details on parsing behavior, see [shellish_parse](https://docs.rs/shellish_parse/latest/shellish_parse/index.html).
