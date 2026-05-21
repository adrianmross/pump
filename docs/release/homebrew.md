# Homebrew Release Path

Pump is distributed through the public `adrianmross/homebrew-tap` tap.

`cargo-dist` generates the release formula as the `pump.rb` GitHub Release
asset. The v0.1.x path is intentionally manual:

```sh
gh release download vX.Y.Z --repo adrianmross/pump --pattern pump.rb --dir /tmp/pump-release
cp /tmp/pump-release/pump.rb /Users/adross/dev/adrianmross/homebrew-tap/Formula/pump.rb
```

Before committing the tap update:

```sh
brew style --fix Formula/pump.rb
brew style Formula/pump.rb
```

Validate through the actual tapped checkout:

```sh
brew audit --formula pump --tap=adrianmross/tap
brew install adrianmross/tap/pump
brew test adrianmross/tap/pump
pump --version
```

The generated formula may need small Homebrew-specific cleanup before commit,
such as removing a redundant `version` line or adding a `test do` smoke test.
