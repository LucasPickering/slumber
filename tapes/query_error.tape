Output "docs/src/images/query_error.gif"
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

Enter # Send login request
Type "hunter2"
Enter
Sleep 2s
Type "rf" # Fullscreen response pane
Show

Type "/"
Sleep 1s
Type "jq error!"
Enter
Sleep 1s
Type "?"
Sleep 3s
