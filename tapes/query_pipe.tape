Output "docs/src/images/query_pipe.gif"
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
Type "2f" # Fullscreen response pane
Wait /to query/

Show
Type "/"
Sleep 1s
Type "jq .[0] | head -n 3"
Enter
Sleep 2s
