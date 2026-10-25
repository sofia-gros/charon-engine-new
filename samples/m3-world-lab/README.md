# M3 World Lab

Tilemapの宝石集めゲームと、砂・水へ変換できるブロックの実装例です。PNGアトラスとWGSLをAssetsから読み込み、汎用描画パケットをwgpuへ渡します。Tilemap・物理・描画の間に直接の必須依存を作りません。

Windows 11（Rust導入済み）でリポジトリのルートから実行します。

```powershell
# Tilemap + Falling Sand
cargo run --release -p charon-m3-sample

# 物理とBridgeを依存に含めないTilemap単体ゲーム
cargo run --release -p charon-m3-sample --no-default-features

# 30Hz計算でも、ゲーム144Hz・描画60Hzの目標周期は独立
cargo run --release -p charon-m3-sample -- --compute-hz 30
```

| 操作 | 結果 |
| --- | --- |
| WASD | 緑色のプレイヤーを移動。黄色い宝石を3個集める |
| 左クリック | タイルを破壊。連携モードでは黄色のブロックは砂、青いブロックは水になる |
| 右クリック | 選択タイルを設置。既存タイルや粒子で塞がれた固体の設置は拒否 |
| 1 / 2 / 3 | 石 / 砂ブロック / 水ブロックを選択 |
| B | 砂・水ブロックをまとめて破壊 |
| G | 中央の石の床を開く |
| L | カスタムShaderの色パラメータを切り替える |
| Esc | workerを停止して終了 |

上段にゲーム・計算tick、宝石数、選択タイルとrevision、下段に最後の編集結果を表示します。`G` → `B` の順に押すと、連携モードでは砂と水が床へ落下します。Tilemap単体ではタイルだけが消え、粒子処理は動きません。Shaderの変更はタイル・粒子・Spriteへ同じ描画契約で適用されます。

起動後にサイズを変更しても描画とマウス座標の対応が保たれます。プレイヤー移動はTilemapの固体を参照する簡単なゲーム側処理です。プレイヤーへの流体の力や浮力は含みません。

開発時の確認コマンド：

```powershell
cargo run -p charon-m3-sample -- --smoke-test
cargo run -p charon-m3-sample --no-default-features -- --smoke-test
cargo run -p charon-m3-sample -- --gpu-check
python scripts/check_m3_dependencies.py
```

`--gpu-check` はウィンドウを開き、GPU初期化・画像転送・独自Shader・不正Shaderの診断と旧パス維持・リサイズ・描画を確認して終了します。画面のピクセル比較や性能ベンチマークではありません。

[APIと境界の説明](../../docs/m3-api.md) / [連携GIF](../../docs/demos/m3-demo.gif) / [Tilemap単体GIF](../../docs/demos/m3-tilemap-demo.gif)

GIFはLinuxの実ウィンドウに入力を送って記録します。対象ソースコミットと操作は同名JSONに保存します。Windows 11実機の入力・表示は未確認です。
