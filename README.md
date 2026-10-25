# charon-engine · [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE) [![CI](https://github.com/sofia-gros/charon-engine-new/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/sofia-gros/charon-engine-new/actions/workflows/ci.yml)

charon-engine は、軽量かつ高性能な Falling Sand エンジンを目指す Rust 製のプロジェクトです。Noita と Terraria に影響を受けています。

- **軽量な物理コア：** 砂・水・火などのセル単位のシミュレーションを中心に、必要な機能を積み上げます。物理コアだけを既存のゲームやアプリケーションに組み込むこともできます。
- **選べる抽象度：** シミュレーションや描画を細かく制御する低レベルAPIと、ゲーム制作を進めやすい高レベルAPIの両方を公開する方針です。
- **Plugins による拡張：** コアが Plugins を読み込み、用途に合わせて機能を追加できる構成を目指します。Falling Sand をゲームの中心にも、背景の演出にも使える自由さを大切にします。

たとえば「Falling Sand を背景に使ったノベルゲーム」。物理シミュレーションに、会話やUIなどを担う Plugins を組み合わせることで、こうしたゲームも作れるエンジンにしていきます。

## 開発状況

現在は、[Sandengine](https://github.com/Ijtihed/sandengine) を土台に、物理コアを独立したライブラリとして取り出した段階です。描画・GUI・入力・履歴保存は切り離してあり、画面なしでシミュレーションを実行できます。

| 機能 | 状況 |
| --- | --- |
| セル単位の物理演算、密度による移動、燃焼・蒸発・溶解などの反応 | 実装済み |
| 2バイトのセルデータ、32×32チャンクによる更新対象の管理 | 実装済み |
| 粒子を押しのける可動ブロック | 実装済み |
| アセットの読み込み・管理 | 計画中 |
| GPU Shader / Shader の取り扱いと描画機能 | 計画中 |
| 低レベルAPIの整備と高レベルAPIの公開 | 物理コアのAPIを公開済み。全体の整備は計画中 |
| Phaser に影響を受けた Camera / Scene などの Core 機能 | 計画中 |
| コアによる Plugins の読み込みと各種拡張機能 | 計画中 |

軽量・高性能はエンジン全体の開発目標です。現在のコアには、毎 tick のセルバッファ全体のコピーなど、最適化の余地があります。また、砂の自然な安息角、セル単位の速度、温度伝導、粘性・表面張力は未実装です。

## 導入

Edition 2024 に対応した最新の stable Rust を使用してください。

現在の crate 名は、既存の物理APIとの互換性を保つため `sandengine` です。

```toml
[dependencies]
sandengine = { git = "https://github.com/sofia-gros/charon-engine-new" }
rand = "0.10.1"
```

## 使用例

グリッドに砂を置き、物理演算を120 tick進める最小の例です。ウィンドウやGPUは必要ありません。

```rust
use rand::{SeedableRng, rngs::StdRng};
use sandengine::sim::{grid::Grid, material::CellData, physics};

let mut grid = Grid::new(365, 365);
let mut rng = StdRng::seed_from_u64(42);
grid.set(182, 10, CellData::sand(0));

for _ in 0..120 {
    physics::step(&mut grid, &mut rng);
}
```

リポジトリ内の [headless サンプル](./examples/headless.rs) も実行できます。

```sh
cargo run --release --example headless
```

## ドキュメント

現在の物理コアは、次のモジュールから構成されています。

- [Grid](./src/sim/grid.rs)：セルの格納、ダブルバッファ、チャンクの更新管理
- [Material](./src/sim/material.rs)：セルの種類、密度、色の取得
- [Physics](./src/sim/physics.rs)：物質ごとの移動と反応
- [Block](./src/sim/block.rs)：可動ブロックと粒子の押し出し

APIドキュメントは `cargo doc --no-deps --open` で確認できます。Camera / Scene や Plugins のAPIは、実装に合わせて整備していきます。

## コントリビュート

バグ報告、改善案、Pull Request を歓迎します。[Issues](https://github.com/sofia-gros/charon-engine-new/issues) で、不具合の再現手順や作りたい機能を共有してください。

変更時は、以下のチェックを実行してください。

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
```

物理処理のベンチマークは `cargo bench` で実行できます。CIでも Linux / macOS / Windows でフォーマット、Clippy、テスト、リリースビルドを確認します。

## ライセンス

[MIT](./LICENSE)。元のコードの著作権表記を保持し、変更分に `Copyright (c) 2026 sofia-gros` を追加しています。
