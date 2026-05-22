Output docs/assets/pump-demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 1080
Set Height 620
Set Padding 18

Type "bat --style=plain --paging=never --color=always --language=json examples/object/source.json"
Enter
Sleep 1s

Type "bat --style=plain --paging=never --color=always --language=yaml examples/object/rules.pump.yaml"
Enter
Sleep 1s

Type "pump diff examples/object/source.json --rules examples/object/rules.pump.yaml | bat --style=plain --paging=never --color=always --language=diff"
Enter
Sleep 1.5s

Type "pump explain examples/object/source.json --rules examples/object/rules.pump.yaml --path '$.api.resources.memory'"
Enter
Sleep 1s

Type "pump diff examples/kubernetes/source.yaml --rules examples/kubernetes/rules.pump.yaml | bat --style=plain --paging=never --color=always --language=diff"
Enter
Sleep 1.5s
