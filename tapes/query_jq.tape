Output "docs/src/images/query_jq.gif"
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
Wait /q Quit/

# Send List Fish request
Down 2
Enter
Sleep 1s
Type "rf" # Fullscreen response pane
Show

Sleep 1s
Type "/"
Sleep 1s
Type "jq .[0]"
Enter
Sleep 2s
Type "/.id"
Enter
Sleep 2s
