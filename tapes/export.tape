Output "docs/src/images/export.gif"
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
Wait /\[q\] quit/

# Send List Fish request
Type "r"
Down 2
Enter
Sleep 1s
Type "2f" # Fullscreen response pane
Show

Sleep 1s
Type ":"
Sleep 1s
Type "jq .[].id > ids.txt"
Sleep 0.5s
Enter
Sleep 2s
