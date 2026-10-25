# charon-engine · [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE) [![CI](https://github.com/sofia-gros/charon-engine-new/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/sofia-gros/charon-engine-new/actions/workflows/ci.yml)

charon-engine は、軽量かつ高性能な、Plugins を組み合わせて使う Rust 製の2Dゲームエンジンを目指すプロジェクトです。Terraria のようなタイルマップ主体のゲームと、Noita のように Falling Sand を組み合わせたゲームの両方を作れる構成を目指します。

- **小さな Core：** Core は Plugin の登録、ライフサイクル、更新順序、イベント・リソースの受け渡しを担当します。物理世界や描画方式を固定しません。
- **独立した部品：** Falling Sand、Tilemap、Sprite、Rendering、Lighting、Assets、Input、Camera / Scene を独立した Plugins として組み合わせます。各機能のロジックをライブラリとして分離し、Plugin は Core への接続を担います。
- **TilemapとFalling Sandの連携：** 任意のBridge Pluginで双方向コリジョンを接続し、ブロックの破壊から砂・水を生成する規則もゲーム側で制御できるAPIを用意します。Tilemapのみ・Falling Sandのみでも利用できる構成を維持します。
- **共有できるライティング：** Tilemap と Falling Sand のどちらからも、共通の遮光・発光データへ変換して照明を計算します。照明の計算と描画への適用も分離します。
- **選べる抽象度：** 各部品を直接使える低レベルAPIと、Plugins を登録してゲームを起動できる高レベルAPIを公開する方針です。

たとえば、Tilemap と Sprite を使う2Dゲーム、Falling Sand を使うサンドボックス、Falling Sand を背景に使ったノベルゲーム。必要な Plugins を選んで構成できるエンジンにしていきます。Falling Sand を使わないゲームも、同じ Core で動かせることを設計条件にします。

標準はマルチスレッド構成です。ゲーム144Hz・レンダリング60Hz・計算30〜60Hzを独立した目標周期とし、各レーンをスナップショットとコマンドで接続します。シングルスレッドは検証・デバッグ向けです。

## 開発状況

大きな目標と実装ごとの小さな目標は、[ロードマップ](./ROADMAP.md)にまとめています。

現在は、[Sandengine](https://github.com/Ijtihed/sandengine) を土台に、Falling Sand の物理処理を独立したライブラリとして取り出した段階です。独立した `charon-core` に、最小のPlugin実行基盤を実装しました。M2ではFalling SandのPlugin接続と外部コリジョン・生成要求のAPIを実装し、性能・物理表現の改善を進めています。描画・GUI・入力・履歴保存は切り離してあり、画面なしでシミュレーションを実行できます。

| 機能 | 状況 |
| --- | --- |
| セル単位の物理演算、密度による移動、燃焼・蒸発・溶解などの反応 | 実装済み |
| 2バイトのセルデータ、32×32チャンクによる更新対象の管理 | 実装済み |
| 粒子を押しのける可動ブロック | 実装済み |
| Falling Sand Plugin、外部コリジョン・生成要求・結果・接触API | M2の最初の実装単位として実装済み |
| 独立した Tilemap / Sprite Plugins と、Falling SandとのBridge | 計画中 |
| Assets Plugin によるアセットの読み込み・管理 | 計画中 |
| Rendering Plugin と Shader、交換可能なGPUバックエンド | 計画中 |
| Lighting Plugin による遮光・光の伝播・AO・発光。Tilemap / Falling Sand の両方に対応 | 計画中 |
| 低レベルAPIの整備と高レベルAPIの公開 | 物理コアのAPIを公開済み。全体の整備は計画中 |
| Phaser に影響を受けた Camera / Scene Plugins | 計画中 |
| Coreによる静的Pluginの登録・依存解決・更新・終了、型付きリソースとイベント | M1実装済み |
| Windows 11向けM1ゲームサンプル（宝石収集ゲーム） | ソース・起動コマンド・自動確認を用意。実機確認待ち |

軽量・高性能はエンジン全体の開発目標です。物理更新時の全セルコピーを削除し、固定seedでの[性能測定基準](./docs/performance/README.md)を用意しました。公開用snapshotのコピーなど、引き続き最適化の余地があります。また、砂の自然な安息角、セル単位の速度、温度伝導、粘性・表面張力は未実装です。

## M実装事例

実際のサンプルウィンドウにキー・マウス入力を送り、画面をGIFとして録画しています。以下はLinuxの仮想ディスプレイ上での記録で、Windows 11実機での確認を示すものではありません。

### M1：Plugin基盤・ゲームサンプル

![M1：移動・宝石取得・一時停止・再開・リセット](./docs/demos/m1-demo.gif)

[録画環境・対象コミット・操作内容](./docs/demos/m1-demo.json)

[Gem Run](./samples/m1-plugin-game/README.md) で、Pluginの組み合わせ、固定tick、イベントによる得点処理を確認できます。Rust導入済みのWindows 11では、リポジトリのルートから次のコマンドで起動できます。操作・実機確認のチェックリストはサンプルのREADMEを参照してください。

```sh
cargo run --release -p charon-m1-sample
```

各マイルストーンの実装完了時に、ゲームサンプルと操作GIFを用意し、GIFをGit管理してこの節に埋め込みます。

### M2：Falling Sand Plugin・物理サンプル（進行中）

![M2進行中：砂・水の配置、障壁の解除、砂への変換、一時停止](./docs/demos/m2-demo.gif)

[録画環境・対象コミット・操作内容](./docs/demos/m2-demo.json)

[Sand Lab](./samples/m2-sand-lab/README.md) で砂・水などを配置し、外部の障壁を解除したり砂へ変換したりできます。

```sh
cargo run --release -p charon-m2-sample
```

[APIと制約](./docs/falling-sand-api.md)を公開しています。安息角・性能最適化は未完了で、M2は進行中です。

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

現在の物理ライブラリは、次のモジュールから構成されています。

- [Grid](./src/sim/grid.rs)：セルの格納、ダブルバッファ、チャンクの更新管理
- [Material](./src/sim/material.rs)：セルの種類、密度、色の取得
- [Physics](./src/sim/physics.rs)：物質ごとの移動と反応
- [Block](./src/sim/block.rs)：可動ブロックと粒子の押し出し

[Core / Pluginの契約](./docs/architecture.md)も参照してください。`charon-core` は物理・描画・ウィンドウに依存しません。

APIドキュメントは `cargo doc --no-deps --open` で確認できます。Camera / Scene や Plugins のAPIは、実装に合わせて整備していきます。

## コントリビュート

バグ報告、改善案、Pull Request を歓迎します。[Issues](https://github.com/sofia-gros/charon-engine-new/issues) で、不具合の再現手順や作りたい機能を共有してください。

変更時は、以下のチェックを実行してください。

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release --workspace
```

物理処理のベンチマークは `cargo bench` で実行できます。CIでも Linux / macOS / Windows でフォーマット、Clippy、テスト、リリースビルドを確認します。

## ライセンス

[MIT](./LICENSE)。元のコードの著作権表記を保持し、変更分に `Copyright (c) 2026 sofia-gros` を追加しています。
