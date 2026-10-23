# Charon Engine

壊せる地形と、遊びを組み合わせる2Dゲームエンジン。

Charon Engineは、サンドボックスや横スクロールアクション、会話を中心としたゲームの制作に向けて開発しているRust製エンジンです。作品に必要な機能を選び、地形の変化・キャラクターの操作・物語を組み合わせられることを目指しています。

[導入と使い方](docs/usage.md) · [サンプル](#サンプル) · [ライセンス](LICENSE)

## サンプル

### Sandbox

砂を降らせて積もらせ、壁を描き、支えを壊して地形の変化を試せます。

![砂を積もらせ、壁を壊して流れを変えるサンドボックス](docs/media/sandbox-linux.gif)

[Sandboxの操作方法](docs/usage.md#sandbox)

### Platformer

砂や壁を足場にして移動・ジャンプ。足元を掘ると支えがなくなり、キャラクターが落下します。

![移動・ジャンプ・足場の破壊を試すアクションサンプル](docs/media/platformer-linux.gif)

[Platformerの操作方法](docs/usage.md#platformer)

### Dialogue

文章を読み進め、選択肢によって物語が分岐する会話サンプルです。

![選択肢によって物語が分岐する会話サンプル](docs/media/dialogue-linux.gif)

[Dialogueの操作方法](docs/usage.md#dialogue)

## 環境構築

Rust stableとGitを用意し、[環境別の導入手順](docs/usage.md#必要な環境)に沿ってリポジトリを取得します。取得したフォルダーで、サンドボックスを起動できます。

```bash
cargo run --locked --release -p charon-sandbox
```

Windowsでの起動、各サンプルの操作、対応範囲は[導入と使い方](docs/usage.md)を参照してください。検証方法は[こちら](docs/usage.md#検証)にまとめています。

## License

[MIT](LICENSE) © 2026 sofia gros
