# Input Bindings

You can customize all input bindings in the configuration. An input binding is a mapping between an action (a high-level verb) and one or more key combinations.

For example if you want wasd controls:

```yaml
# config.yml
input_bindings:
  up: [w]
  down: [s]
  left: [a]
  right: [d]
```

Each action maps to a _list_ of key combinations, because you can map multiple combinations to a single action. Hitting any of these combinations will trigger the action. By defining a binding in the config, **you will replace the default binding for that action**. If you want to retain the default binding but add an additional binding as well, you will need to include the default in your list of custom bindings. For example, if you want wasd bindings _and_ the default arrow keys:

```yaml
# config.yml
input_bindings:
  up: [up, w]
  down: [down, s]
  left: [left, a]
  right: [right, d]
```

## Actions

| Action                | Default Binding         | Description                                                                                                                       |
| --------------------- | ----------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `scroll_up`           | `shift up`/`shift k`    | Scroll up one line in the current list/view                                                                                       |
| `scroll_down`         | `shift down`/`shift j`  | Scroll up one line in the current list/view                                                                                       |
| `scroll_left`         | `shift left`/`shift h`  | Scroll left one column in the current view                                                                                        |
| `scroll_right`        | `shift right`/`shift l` | Scroll right one column in the current view                                                                                       |
| `quit`                | `q`                     | Exit current dialog, or the entire app                                                                                            |
| `force_quit`          | `ctrl c`                | Exit the app, regardless                                                                                                          |
| `previous_pane`       | `shift tab`             | Select previous pane/form field in the cycle                                                                                      |
| `next_pane`           | `tab`                   | Select next pane/form field in the cycle                                                                                          |
| `up`                  | `up`/`k`                | Navigate up                                                                                                                       |
| `down`                | `down`/`j`              | Navigate down                                                                                                                     |
| `left`                | `left`/`h`              | Navigate left                                                                                                                     |
| `right`               | `right`/`l`             | Navigate right                                                                                                                    |
| `page_up`             | `pgup`                  | Scroll up by one page                                                                                                             |
| `page_down`           | `pgdn`                  | Scroll down by one page                                                                                                           |
| `home`                | `home`                  | Move to the start of a line of text                                                                                               |
| `end`                 | `end`                   | Move to the end of a line of text                                                                                                 |
| `submit`              | `enter`                 | Send a request, submit a text box, etc.                                                                                           |
| `toggle`              | `space`                 | Toggle a checkbox on/off                                                                                                          |
| `cancel`              | `esc`                   | Cancel current dialog or request                                                                                                  |
| `delete`              | `delete`                | Delete the selected object (e.g. a request)                                                                                       |
| `edit`                | `e`                     | Edit a template or form field                                                                                                     |
| `reset`               | `r`                     | Reset temporary recipe override to its default                                                                                    |
| `view`                | `v`                     | Open the selected content (e.g. body) in your pager                                                                               |
| `history`             | `ctrl h`                | Open request history for a recipe                                                                                                 |
| `search`              | `/`                     | Open/select search for current pane                                                                                               |
| `export`              | `:`                     | Enter command for exporting response data                                                                                         |
| `reload_collection`   | `f5`                    | Force reload collection file                                                                                                      |
| `fullscreen`          | `f`                     | Fullscreen current pane                                                                                                           |
| `open_actions`        | `x`                     | Open actions menu                                                                                                                 |
| `open_help`           | `?`                     | Open help page                                                                                                                    |
| `search_history`      | `ctrl r`                | Search command history in query/export text box                                                                                   |
| `select_bottom_pane`  | `2`                     | Select the lower pane (Request/Response or Profile). Aliased to `select_request` and `select_response` for backward compatibility |
| `select_collection`   | `f3`                    | Open collection select dialog                                                                                                     |
| `select_profile_list` | `p`                     | Open Profile List dialog                                                                                                          |
| `select_recipe_list`  | `r`                     | Select Recipe List pane                                                                                                           |
| `select_top_pane`     | `1`                     | Select the upper pane (the recipe pane). Aliased to `select_recipe` for backward compatibility                                    |

## Key Combinations

A key combination consists of zero or more modifiers, followed by a single key code. The modifiers and the code all each separated by a single space. Some examples:

- `w`
- `shift f2`
- `alt shift c`
- `ctrl alt delete`

### Key Codes

All single-character keys (e.g. `w`, `/`, `=`, etc.) are not listed; the code is just the character.

- `escape`/`esc`
- `enter`
- `left`
- `right`
- `up`
- `down`
- `home`
- `end`
- `pageup`/`pgup`
- `pagedown`/`pgdn`
- `tab`
- `backtab` (equivalent to `shift tab`, supported for backward compatibility)
- `backspace`
- `delete`/`del`
- `insert`/`ins`
- `capslock`/`caps`
- `scrolllock`
- `numlock`
- `printscreen`
- `pausebreak` (sometimes just called Pause; _not_ the same as the Pause media key)
- `menu`
- `keypadbegin`
- `f1`
- `f2`
- `f3`
- `f4`
- `f5`
- `f6`
- `f7`
- `f8`
- `f9`
- `f10`
- `f11`
- `f12`
- `space`
- `play`
- `pause` (the media key, _not_ Pause/Break)
- `playpause`
- `reverse`
- `stop`
- `fastforward`
- `rewind`
- `tracknext`
- `trackprevious`
- `record`
- `lowervolume`
- `raisevolume`
- `mute`

### Key Modifiers

- `shift`
- `alt`
- `ctrl`
- `super`
- `hyper`
- `meta`
