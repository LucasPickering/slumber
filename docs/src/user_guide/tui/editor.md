# In-App Editing & File Viewing

## Editing

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

The command will be parsed like a shell command (although a shell is never actually invoked). For exact details on parsing behavior, see [shell-words](https://docs.rs/shell-words/1.1.0/shell_words/fn.split.html).

## Paging

You can open request and response bodies in a separate file browser if you want additional features beyond what Slumber provides. To configure the command to use, set the `PAGER` environment variable or the `pager` configuration field:

```yaml
pager: bat
```

> The pager command uses the same format as the `editor` field. The command is parsed with [shell-words](https://docs.rs/shell-words/1.1.0/shell_words/fn.split.html), then a temporary file path is passed as the final argument.

To open a body in the pager, use the actions menu keybinding (`x` by default, see [input bindings](./input_bindings.md)), and select `View Body`.

Some popular file viewers:

- [bat](https://github.com/sharkdp/bat)
- [fx](https://fx.wtf/)
- [jless](https://github.com/PaulJuliusMartinez/jless)
