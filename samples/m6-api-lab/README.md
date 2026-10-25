# M6 API Lab

同じゲームを、部品の直接APIと高レベル構成APIで起動します。M5のゲームロジックとウィンドウホストを共有し、構成の違いは [setup.rs](./src/setup.rs) にまとめています。

Windows 11（Rust導入済み）で、リポジトリのルートから実行します。

```powershell
# 高レベルAPI：Startup + Pluginセット + SceneCatalog
cargo run --release -p charon-m6-sample -- --api high

# 低レベルAPI：App + 個別Plugin + SceneManager
cargo run --release -p charon-m6-sample -- --api low

# Falling Sandを依存に含めず、Tilemapと照明だけを使用
cargo run --release -p charon-m6-sample --no-default-features --features lighting -- --api high

# LightingもFalling Sandも使わない構成
cargo run --release -p charon-m6-sample --no-default-features -- --api low
```

Enterで開始、WASDでプレイヤーを移動、矢印でCamera移動、Q/E・ホイールでズーム、Spaceで全体表示します。左クリックで破壊、右クリックで設置、Bで砂・水ブロックを解放、Rで再開始、Tabでメニューへ戻ります。メニューのTはTilemap単体を開始します。

Uは鉱石のworld座標(252,132)のGPU照明を明示readbackし、選択中のAPIでRGBを表示します。Zで解像度を切り替え、もう一度Uで取得できます。ワールド変更後は再取得が必要と表示します。照明非搭載構成ではこの操作は利用しません。照明のreadbackは診断目的で、通常の描画には不要です。

ゲーム144Hz・描画要求60Hz・計算30〜60Hzは独立した目標周期です。`--compute-hz 30` も使えます。ゲーム・計算workerを使用し、描画とOS入力はevent-loop threadで処理します。

## 検証

```powershell
cargo run -p charon-m6-sample -- --smoke-test
cargo run -p charon-m6-sample -- --api high --gpu-check
cargo run -p charon-m6-sample -- --api low --gpu-check
python scripts/check_m6_dependencies.py
```

smokeはHIGH/LOWの初期world・Cameraの一致に加え、それぞれの入力による掘削、再開始、旧入力口の切断、アセット・workerの解放を確認します。Tilemap単体／連携の計算30/60Hzで実行します。GPU確認は実ウィンドウとInputClientへのテスト入力を使い、world座標の照明取得、Camera・掘削・再開始・GPUリソース解放を確認します。

操作GIFはLinux / Xvfb上で実際のOS入力を送って録画します。Windows 11実機での表示・入力は未確認です。

- [ ] HIGH/LOWで同じworld・入力・掘削・Camera操作を使える。
- [ ] Uで照明RGBを取得し、Zで解像度を変えて再取得できる。
- [ ] R・Tabを繰り返しても、旧Sceneの状態や押していた入力が残らない。
- [ ] Falling Sandなし／照明なしでも起動できる。

[公開API・移行ガイド](../../docs/api-guide.md) / [charon_engine Feature一覧](../../crates/charon_engine/README.md)
