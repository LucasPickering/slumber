# Theme

Theming allows you to customize the appearance of the Slumber TUI. To start, [open up your configuration file](../configuration/index.md#location--creation) and add some theme settings:

```yaml
theme:
  primary_color: green
  secondary_color: blue
```

## Fields

| Field                            | Type     | Description                                                          |
| -------------------------------- | -------- | -------------------------------------------------------------------- |
| `primary_color`                  | `Color`  | Color of most emphasized content                                     |
| `primary_text_color`             | `Color`  | Color of text on top of the primary color (generally white or black) |
| `secondary_color`                | `Color`  | Color of secondary notable content                                   |
| `success_color`                  | `Color`  | Color representing successful events                                 |
| `error_color`                    | `Color`  | Color representing error messages                                    |
| `background_color`               | `Color`  | Color of the background of the application                           |
| `inactive_color`                 | `Color`  | Color of inactive text and components                                |
| `text_color`                     | `Color`  | Color of regular text                                                |
| `hint_text_color`                | `Color`  | Color of hint text                                                   |
| `textbox_background_color`       | `Color`  | Background color of text boxes                                       |
| `cursor_background_color`        | `Color`  | Background color of the cursor                                       |
| `cursor_text_color`              | `Color`  | Text color of the cursor                                             |
| `gutter_background_color`        | `Color`  | Background color of the gutter                                       |
| `gutter_text_color`              | `Color`  | Text color of the gutter                                             |
| `alternate_row_background_color` | `Color`  | Background color of alternate table rows                             |
| `alternate_row_text_color`       | `Color`  | Text color of alternate table rows                                   |
| `syntax`                         | `Object` | Visual configuration for the syntax highlighting (see below)         |

### Syntax Highlighting Fields

| Field           | Type    | Description                  |
| --------------- | ------- | ---------------------------- |
| `builtin_color` | `Color` | Color for builtins           |
| `comment_color` | `Color` | Color for comments           |
| `escape_color`  | `Color` | Color for escape characters  |
| `number_color`  | `Color` | Color for numbers            |
| `special_color` | `Color` | Color for special characters |
| `string_color`  | `Color` | Color for strings            |

## Color Format

Colors can be specified as names (e.g. "yellow"), RGB codes (e.g. `#ffff00`) or ANSI color indexes. See the [Ratatui docs](https://docs.rs/ratatui/latest/ratatui/style/enum.Color.html#impl-FromStr-for-Color) for more details on color deserialization.
