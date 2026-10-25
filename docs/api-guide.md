# M6：直接APIと構成API

各ライブラリを直接呼ぶAPIは維持し、任意の `charon_engine` が起動設定・Pluginセット・Scene登録をまとめます。高レベルAPIも同じApp / World / SceneManagerを返します。ゲームのworld型、描画方式、OSイベントループは指定しません。

## 部品の公開範囲

| crate | 直接利用 | Coreへの接続 |
| --- | --- | --- |
| charon_core | App、World、Worker、固定更新とframe更新 | Pluginトレイトと依存解決 |
| charon_physics | Simulation、物質登録、グリッド、コリジョン | charon_sandが任意接続 |
| charon_tilemap | タイル・Sprite・変更履歴 | plugin feature |
| charon_assets | 型付きAssetsとハンドル・読込関数 | AssetsPlugin |
| charon_render | DrawFrame、Quad、Backend | RenderingPlugin。gpu featureは明示選択 |
| charon_lighting | 光学Scene・レイヤー・光源・CPU参照結果 | plugin feature。GPU計算は別crate |
| charon_input / charon_camera / charon_scene | 入力状態・変換・シーン遷移 | 各plugin feature |
| charon_engine | 上記の任意再公開、照明設定と結果取得 | AppBuilder・Pluginセット・SceneCatalog |

Tilemap/Sandの描画、光学入力、Cameraの描画適用、winit入力、Bridgeも別crateです。Coreへ依存を逆流させません。独立した物理・Tilemap・光学ライブラリを利用するときは、facadeへの依存は不要です。

## 起動設定と独立した周期

```rust
use charon_engine::{Startup, Rates, Lane};
let startup = Startup::new(Rates::new(144, 60, 30)?, 8)?;
let game = startup.app(Lane::Game)?.build();
let compute = startup.app(Lane::Compute)?.build();
let render_interval = startup.rates().delta(Lane::Render);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Rates`は1〜1000Hz、catch-up予算は1〜1024に制限します。既定はゲーム144Hz・描画60Hz・計算60Hzです。各Appは別のWorldを所有し、Worker間は既存の有界コマンド・スナップショットで接続します。指定した周期での性能達成を保証する値ではありません。

`Startup::spawn(Lane::Game / Compute, name, configure, hooks)` はowner thread内でAppBuilderを作り、configureが返したAppを初期化します。Workerのstop/joinとDrop時の停止は既存Coreの契約です。描画はプラットフォームのevent-loop threadで扱うため、Renderレーンのspawnは拒否します。ウィンドウの作成・入力転送・frame要求はプラットフォームホストが担当します。

## Pluginセットと低レベルへのアクセス

```rust
use charon_engine::{AppBuilder, Startup, bundles::Interaction2D};
use charon_engine::camera::{Camera2D, Rect};
let camera = Camera2D::new([160., 96.], 3., Rect { x: 0., y: 72., width: 960., height: 576. })?;
let set = Interaction2D::new(camera, 256)?;
let input = set.client();
let mut builder = AppBuilder::new(Startup::default())?.bundle(set)?;
// 独自Pluginはbuilder.plugin(plugin)?で追加する。
// app_mut()では既存Core API、world_mut()では型付きリソースを直接扱える。
let mut app = builder.build();
app.start()?;
app.shutdown()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Interaction2DはCPU Assets・Input・Cameraを登録し、InputClientをWorldにも格納します。GPU・Lighting・Tilemap・Sandは登録しません。Assetsだけなら `bundles::Assets`、個別指定なら `.plugin(AssetsPlugin::default())` などを使います。

`.resource(value)` は構成時のWorldへ値を挿入します。同じ型のリソースは既存Worldと同じく置き換えます。`.plugin()` と `.bundle()` はbuilderを消費し、失敗した構成をそのまま開始できない形にしています。Plugin名の重複、依存不足、依存循環はCoreの診断を保ちます。Featureを有効にするだけではPluginは登録されません。

## Sceneカタログ

`SceneCatalog::new(startup)?.scene(name, factory)?.build(initial)` でSceneManagerを作ります。factoryはSceneIdと、managerと同じゲーム周期の新しいAppBuilderを受け取り、`Box<dyn Scene>` を返します。ScopedSceneでAppとSceneLogicを接続できます。

factoryは再開始のたびに呼ばれます。停止したAppや旧Sceneのアセットハンドルを再利用しません。`manager_mut()` から直接登録もできます。buildの結果は既存SceneManagerで、開始・停止・破棄、失敗時の旧Scene保持、古いSceneId拒否はM5と同じ契約です。

具体的な高レベル構成と直接構成は [setup.rs](../samples/m6-api-lab/src/setup.rs) に並べています。

## Lighting：品質・光源・光学特性・結果取得

`charon_engine` の `lighting` featureだけで利用でき、GPUや物理には依存しません。

- `LightingConfig::for_bounds([x,y,width,height], Quality::Low/High)`：Lowは4 world units/セル・30Hz、Highは2 units/セル・60Hz。
- `LightingConfig::new(geometry, settings, hz)`：任意の解像度・原点・伝播距離・AO・環境光・更新周期を設定。
- `Lighting::set_layer(id, OpticalGrid)`：遮光・透過・発光を共通光学レイヤーとして設定。
- `set_light / remove_light`：任意位置・色・強度の光源を更新。
- `set_settings / set_update_hz`：伝播・AO・空の光と計算頻度を変更。
- `scene / scene_mut / prepare`：低レベルの光学APIとGPU提出形式へアクセス。
- `solve_cpu`：参照計算。`accept(LightField)`：別バックエンドの結果を検証して受け取る。
- `sample_world([x,y])`：同じworld原点・単位で、現在のセルのRGBA（RGB照明とAのAO）を取得。

解像度を変更する場合は、新しいconfigからLightingを作り、同じworldの光学入力・光源を再提出します。解像度の違うグリッドやGPU結果をそのまま混在させません。端のセルはboundsを超える場合があるため、厳密な範囲が必要ならgeometryを明示してください。

ワールド変更でrevisionが変わると、旧fieldは `field()` / `sample_world()` から取得できなくなります。更新待ち・範囲外・不正座標ではNoneです。非同期の旧revisionはacceptがfalseを返し、不正なgeometry・配列長・非有限値はエラーとなり現在のキャッシュを維持します。revisionはScene内の値なので、別Sceneの同revision結果を渡さないよう、ジョブと結果の所有権をSceneごとに分けます。

ライブGPU計算は `prepare()` を `charon_lighting_gpu` へ渡します。通常の描画に同期readbackは不要です。CPUからGPU照明を読む場合は明示的なreadbackが必要で、待機・転送コストがかかります。比較ゲームのUキーはこの診断用操作です。毎frameのreadbackは行いません。

`CpuLightingPlugin::new(lighting)` は参照計算を更新周期に合わせて行います。Computeレーンへ明示登録してください。CPU参照とGPU計算を同じworldに重複登録する必要はありません。

## crate名・導入・移行

エンジンcrateは `charon_{機能名}` に統一します。M6ではCargo package名とディレクトリの `charon-core` → `charon_core`、`charon-sand` → `charon_sand` を変更しました。Rustの `use charon_core` / `use charon_sand` は同じです。古いCargo依存名・pathと `cargo -p` は変更してください。ゲームサンプルの `charon-mN-sample` コマンドは維持します。

個別部品はこれまで通りgit/path依存で利用でき、全体構成を使う場合だけ `charon_engine` を追加します。Feature一覧は [crate README](../crates/charon_engine/README.md) にあります。Coreのみのfacadeは既定でCore以外を読み込みません。crates.io公開は未実施です。開発版APIなので、利用側では動作確認したコミットをrevで固定してください。

APIドキュメント：`cargo doc --workspace --all-features --no-deps --open`。CIではrustdocの警告をエラー扱いにし、依存グラフと各構成も検証します。M1〜M5のサンプルは引き続き直接APIを使い、移行を強制しません。
