# Shell Completions

Slumber provides tab completions for most shells. For the full list of supported shells, [see the clap docs](https://docs.rs/clap_complete/latest/clap_complete/aot/enum.Shell.html).

> Note: Slumber uses clap's native shell completions, which are still experimental. [This issue](https://github.com/clap-rs/clap/issues/3166) outlines the remaining work to be done.

To source your completions:

## Bash

```bash
echo "source <(COMPLETE=bash slumber)" >> ~/.bashrc
```

## Elvish

```elvish
echo "eval (E:COMPLETE=elvish slumber | slurp)" >> ~/.elvish/rc.elv
```

## Fish

```fish
echo "source (COMPLETE=fish slumber | psub)" >> ~/.config/fish/config.fish
```

## Powershell

```powershell
echo "COMPLETE=powershell slumber | Invoke-Expression" >> $PROFILE
```

## Zsh

```zsh
echo "source <(COMPLETE=zsh slumber)" >> ~/.zshrc
```
