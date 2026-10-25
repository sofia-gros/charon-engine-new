# M4 Cave Light

洞窟を掘り、松明・発光鉱石・遮光・AOを確認するゲームサンプルです。TilemapとFalling Sandは共通の光学データへ変換し、GPU Computeで照明を計算します。描画への合成は別のShaderです。

Windows 11（Rust導入済み）でリポジトリのルートから実行します。

```powershell
# Tilemap + Falling Sand + Lighting
cargo run --release -p charon-m4-sample

# Falling Sandを依存に含めず、同じLightingをTilemapで使う
cargo run --release -p charon-m4-sample --no-default-features --features lighting

# LightingもFalling Sandも含めない描画
cargo run --release -p charon-m4-sample --no-default-features

# 独立した周期と照明の品質を指定
cargo run --release -p charon-m4-sample -- --compute-hz 60 --lighting-hz 30 --light-cell-size 4
```

| 操作 | 結果 |
| --- | --- |
| WASD | プレイヤーと松明を動かし、黄色い宝石を集める |
| 左クリック / 右クリック | タイルを掘る / 選択タイルを設置する |
| 1 / 2 / 3 | 石 / 砂ブロック / 水ブロックを選択 |
| T | 持っている松明の発光を切り替える |
| O | 右側の鉱石の発光を切り替える |
| G | 中央の壁の通路を開閉する。粒子で塞がれた設置は拒否 |
| B | 砂・水ブロックを破壊する。連携モードでは粒子へ変換 |
| F | 連携モードで発光する火の粒子を生成する |
| V | 接触部・隙間を暗くするAOを切り替える |
| K | 空からの光を切り替える |
| Q | 照明セルの大きさ4 / 2を切り替える |
| L | 照明合成を切り替え、同じ描画を照明なしでも確認する |
| Esc | workerを停止して終了 |

最初は左右の洞窟が壁で分かれています。Tで松明、Oで鉱石を消すと、光源がなくなった側が暗くなります。Gで通路を開閉し、光の回り込みと遮光を確認できます。Bで砂・水を放出すると、粒子の遮光・透過も照明へ反映されます。Fの火は動く発光源になります。

各タイルや材料の光学特性は、ゲーム側のアダプター設定です。物理やTilemapのデータ型へ照明処理を組み込んでいません。

照明の領域は320×192ワールド単位、標準の照明セルは4単位なので80×48です。Lで照明合成を外しても、タイル編集・物理・Sprite・UIは動きます。Torchなどの操作は `lighting` featureを有効にした構成で使えます。

検証コマンド：

```powershell
cargo run -p charon-m4-sample -- --smoke-test
cargo run -p charon-m4-sample -- --gpu-check
cargo run --release -p charon-lighting-check -- m4-report.json m4-images
python scripts/check_m4_dependencies.py
```

`--gpu-check` は実ウィンドウでCompute結果とCPU参照値、照明テクスチャの共有・合成、リサイズ、描画を確認して終了します。`charon-lighting-check` はウィンドウを開かず、42ケースの数値比較と、指定時にはCPU/GPUのPNG比較を行います。

[照明API](../../docs/lighting-api.md) / [計測と制約](../../docs/performance/m4-lighting.md) / [連携GIF](../../docs/demos/m4-demo.gif) / [Tilemap単体GIF](../../docs/demos/m4-tilemap-demo.gif)

Windows 11実機での表示・入力は未確認です。操作GIFはLinux GitHub Actions / Xvfbの実ウィンドウとXTest入力で録画し、ソースのコミットと操作を同名JSONへ記録します。
