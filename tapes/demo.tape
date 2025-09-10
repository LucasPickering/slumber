Output "docs/src/images/demo.gif"
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
Type "l"
Show

# Send login request
Sleep 1s
Enter
Sleep 2s
Down 2

# Edit param for list_fish
Sleep 1s
Type "c"
Sleep 0.5s
Right
Sleep 0.5s
Type "e"
Sleep 0.5s
Backspace 3
Sleep 0.5s
Type "baz"
Sleep 0.5s
Enter
Sleep 0.5s

# Send list_fish request
Enter
Type "r"
Sleep 2s

# Search response
Type "/jq .[0]"
Enter
Sleep 2s
