Output docs/assets/pump-demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 960
Set Height 560
Set Padding 18

Type "pump --help"
Enter
Sleep 1s

Type "pump check examples/object/source.json --rules examples/object/rules.pump.yaml"
Enter
Sleep 1s

Type "pump diff examples/object/source.json --rules examples/object/rules.pump.yaml"
Enter
Sleep 1s

Type "cp examples/object/source.json /tmp/pump-demo.json"
Enter
Sleep 400ms

Type "pump check /tmp/pump-demo.json --rules examples/object/rules.pump.yaml --write"
Enter
Sleep 1s

Type "pump check /tmp/pump-demo.json --rules examples/object/rules.pump.yaml --strict"
Enter
Sleep 1s

Type "pump explain examples/object/source.json --rules examples/object/rules.pump.yaml --path '$.second.bottom'"
Enter
Sleep 1s
