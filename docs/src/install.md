# Install

Slumber binaries are available from the [GitHub Releases page](https://github.com/LucasPickering/slumber/releases). Or if you prefer a managed installation:

### cargo

```sh
cargo install slumber --locked
```

### cargo binstall

```sh
cargo binstall slumber
```

### homebrew

```sh
brew install LucasPickering/tap/slumber
```

### sh

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/LucasPickering/slumber/releases/download/v3.3.0/slumber-installer.sh | sh
```

### powershell

```sh
powershell -c "irm https://github.com/LucasPickering/slumber/releases/download/v3.3.0/slumber-installer.ps1 | iex"
```

## Shell Completions

After installation, you can optionally install shell completions for TAB-complete of CLI commands. For the full list of supported shells, [see the clap docs](https://docs.rs/clap_complete/latest/clap_complete/aot/enum.Shell.html).

> Note: Slumber uses clap's native shell completions, which are still experimental. [This issue](https://github.com/clap-rs/clap/issues/3166) outlines the remaining work to be done.

To source your completions:

### Bash

```sh
echo "source <(COMPLETE=bash slumber)" >> ~/.bashrc
```

### Elvish

```sh
echo "eval (E:COMPLETE=elvish slumber | slurp)" >> ~/.elvish/rc.elv
```

### Fish

```sh
echo "source (COMPLETE=fish slumber | psub)" >> ~/.config/fish/config.fish
```

### Powershell

```sh
echo "COMPLETE=powershell slumber | Invoke-Expression" >> $PROFILE
```

### Zsh

```sh
echo "source <(COMPLETE=zsh slumber)" >> ~/.zshrc
```
