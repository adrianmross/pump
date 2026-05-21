Output docs/assets/pump-demo.gif

Set Shell "bash"
Set FontSize 16
Set Width 960
Set Height 560
Set Padding 18

Type "pump --help"
Enter
Sleep 1s

Type "pump inflate examples/object/source.json --rules examples/object/rules.pump.yaml"
Enter
Sleep 1s

Type "pump explain examples/object/source.json --rules examples/object/rules.pump.yaml --path '$.second.bottom'"
Enter
Sleep 1s
