# Input Bindings

You can customize all input bindings in the configuration. An input binding is a mapping between an action (a high-level verb) and one or more key combinations.

For example if you want vim bindings (h/j/k/l instead of left/down/up/right):

```yaml
# config.yaml
input_bindings:
  up: [k]
  down: [j]
  left: [h]
  right: [l]
  scroll_left: [shift h]
  scroll_right: [shift l]
  select_recipe_list: [p] # Rebind from `l`
```

Each action maps to a _list_ of key combinations, because you can map multiple combinations to a single action. Hitting any of these combinations will trigger the action. By defining a binding in the config, **you will replace the default binding for that action**. If you want to retain the default binding but add an additional, you will need to include the default in your list of custom bindings. For example, if you want vim bindings but also want to leave the existing arrow key controls in place:

```yaml
input_bindings:
  up: [up, k]
  down: [down, j]
  left: [left, h]
  right: [right, l]
  scroll_left: [shift left, shift h]
  scroll_right: [shift right, shift l]
  select_recipe_list: [p] # Rebind from `l`
```

## Actions

| Action                | Default Binding             |
| --------------------- | --------------------------- |
| `left_click`          | None                        |
| `right_click`         | None                        |
| `scroll_up`           | None                        |
| `scroll_down`         | None                        |
| `scroll_left`         | `shift left`                |
| `scroll_right`        | `shift right`               |
| `quit`                | `q`                         |
| `force_quit`          | `ctrl c`                    |
| `previous_pane`       | `backtab` (AKA `shift tab`) |
| `next_pane`           | `tab`                       |
| `up`                  | `up`                        |
| `down`                | `down`                      |
| `left`                | `left`                      |
| `right`               | `right`                     |
| `page_up`             | `pgup`                      |
| `page_down`           | `pgdn`                      |
| `home`                | `home`                      |
| `end`                 | `end`                       |
| `submit`              | `enter`                     |
| `cancel`              | `esc`                       |
| `history`             | `h`                         |
| `search`              | `/`                         |
| `reload_collection`   | `f5`                        |
| `fullscreen`          | `f`                         |
| `open_actions`        | `x`                         |
| `open_help`           | `?`                         |
| `select_profile_list` | `p`                         |
| `select_recipe_list`  | `l`                         |
| `select_recipe`       | `c`                         |
| `select_request`      | `r`                         |
| `select_response`     | `s`                         |

> Note: mouse bindings are not configurable; mouse actions such as `left_click` _can_ be bound to a key combination, which cannot be unbound from the default mouse action.

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
- `backtab`
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
