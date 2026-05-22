Output docs/assets/pump-demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 1080
Set Height 620
Set Padding 18

Type "cd examples/object"
Enter
Sleep 500ms

Type "alias b='bat --style=plain --paging=never --color=always'"
Enter
Sleep 500ms

Type "b --language=json source.json"
Enter
Sleep 1s

Type "b --language=yaml rules.pump.yaml"
Enter
Sleep 1s

Type "pump diff source.json --rules rules.pump.yaml | b --language=diff"
Enter
Sleep 1.5s

Type "pump explain source.json --rules rules.pump.yaml --path '$.api.resources.memory'"
Enter
Sleep 1s

Type "cd ../kubernetes"
Enter
Sleep 500ms

Type "pump diff source.yaml --rules rules.pump.yaml | b --language=diff"
Enter
Sleep 1.5s
