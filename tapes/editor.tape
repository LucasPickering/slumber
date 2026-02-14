Output "docs/src/images/editor.gif"
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
Show

Sleep 1s
Type "x"
Sleep 1s
Enter
Sleep 2s
# Don't write to the file because that triggers a reload, which can affect other
# gifs being rendered concurrently
Type ":q"
Enter
Sleep 1s
