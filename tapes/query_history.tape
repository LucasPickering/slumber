Output "docs/src/images/query_history.gif"
Set Shell bash
Set Width 1200
Set Height 800
Set FontSize 18
Set Framerate 24
Set Margin 0
Set Padding 10
Set Theme "MaterialDark"

Hide
Type "cargo run"
Enter
Sleep 2s

# Send List Fish request
Down 2
Enter
Sleep 1s
Type "rf" # Fullscreen response pane

# Put some commands in history
Type "/jq .[0]"
Enter
Type "/"
Backspace 10
Type "jq .[1]"
Enter
Type "/"
Backspace 10
Type "jq .[2]"
Enter
Type "/"
Backspace 10
Type "head -n 5"
Enter
# Leave text box empty
Type "/"
Backspace 10
Enter

Show
Type "/"
Sleep 1s
Ctrl+R
Sleep 1s
Type "jq"
Sleep 1s
Up
Sleep 1s
Enter
Sleep 2s
