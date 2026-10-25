# M3：Assets / Rendering / Tilemap / Bridge

世界データ、描画データ、GPUリソースを別々に所有します。Coreに物理・Tilemap・GPUへの依存は追加しません。

| crate | 役割 | 任意の接続 |
| --- | --- | --- |
| `charon_assets` | 型付き世代ハンドル、PNGのCPUデコード、Shaderソース、読み込み・解放・エラー通知 | `AssetsPlugin::default()` |
| `charon_tilemap` | TileDefinition、Tilemap編集と変更履歴、Atlas、Sprite | `plugin` featureで `TilemapPlugin` |
| `charon_render` | 汎用Quad / DrawFrame、Backend、RenderingPlugin | `gpu` featureでwgpu / winit |
| `charon_tilemap_render` | Tilemap / Sprite → 汎用描画パケット | 物理もGPUも必須にしない |
| `charon_sand_render` | 物理Snapshot → CPU RGBA画像とQuad | Tilemapを参照しない |
| `charon_bridge` | Tilemapと物理を計算境界でまとめて更新するBridge / BridgePlugin | 単体のTilemap・物理を変更しない |

## アセットとGPU

`Assets<Image>::load(path, Image::decode)` はPNGをRGBAへ、`Assets<ShaderSource>::load(path, ShaderSource::decode)` はUTF-8のソースへ変換します。`get(handle)` は `Arc<T>` を返します。`unload(handle)` 以降、同じスロットを再利用しても旧ハンドルは無効です。既存のArcを持つ処理は読み終えられます。`drain_events()` で Loaded / Unloaded / Failed を取り出します。

読み込みは呼び出し元スレッドで同期的に行います。起動時、またはアセット用workerで実行してください。ファイル監視・非同期ロード・ホットリロードは今後の対象です。

GPU転送は独立した `GpuRenderer::upload(TextureId, &Image)` で行い、GPU側の解放は `remove_texture` です。CPUアセットの解放で使用中のGPU画像を勝手に消しません。TextureIdは描画ホスト側で重複しないよう管理し、0は白の組み込みテクスチャに予約します。

## Tilemap / Sprite

`Tilemap::new(width, height, [tile_width, tile_height])` の寸法はゲームごとに設定できます。8×8はサンプルの選択です。`register(TileDefinition)` で名前、固体判定、アトラス番号、色を登録します。`set(x,y,Some(id))` で設置し、`None` で削除します。不正なIDや座標はエラーです。

実際に値が変わった編集だけがrevisionを進め、`Change { before, after, x, y, revision }` を記録します。利用側は計算境界で `drain_changes()` を呼びます。履歴を残し続けないでください。任意の `TilemapPlugin` は変更をCoreイベントへ移し、次tickで前回分のイベントを消します。

Atlasには画像サイズとタイルサイズ、Spriteには位置・サイズ・アトラス番号・レイヤーを指定します。アダプター `append_tiles` / `append_sprite` が汎用Quadを生成します。RenderingはTilemapの内部構造を知りません。

## 描画とShader

`DrawFrame` はサイズ、背景色、Quadの一覧です。Quadには矩形、UV、色、TextureId、layer、PassIdを指定します。`Backend` を実装すれば別の描画先にも接続できます。`RenderingPlugin` はCoreのframeステージでDrawFrameをBackendへ渡します。GPU初期化とウィンドウの所有はホストのメインスレッドです。

wgpuバックエンドは、登録順の描画パス内でlayerの昇順を保ち、連続した同一テクスチャをまとめて描画します。`register_pass(PassId, &ShaderSource, [f32;4])` はWGSLを検証してパイプラインへ変換します。失敗すると位置情報を含む診断を返し、置き換え前のパスを維持します。`set_parameters` は再コンパイルせずuniformを更新します。

Shaderの契約は [quad.wgsl](../crates/charon_render/src/quad.wgsl) を参照してください。頂点入力はposition(vec2)、uv(vec2)、tint(vec4)、エントリは `vs_main` / `fs_main`。group 0 は画像とsampler、group 1 はvec4のパラメータです。初期化失敗・サイズ上限・不正な描画参照はResultで通知します。surface Lost / Outdated は再構成、Timeoutはそのフレームをスキップ、OutOfMemoryは終了可能なエラーとして返します。

M3のサンプルShaderは色パラメータの接続例です。遮光・光の伝播・発光・AOを備えるライティングはM4で独立した計算と合成として追加します。

## Tilemap / Falling Sand連携

```rust
use charon_bridge::{Bridge, Command, Edit};
use charon_physics::sim::{material::CellData, simulation::Simulation};
use charon_tilemap::{Tilemap, TileDefinition};

let mut map = Tilemap::new(40, 24, [8, 8])?;
let sand_tile = map.register(TileDefinition {
    name: "Sand block".into(), solid: true,
    atlas_index: 1, tint: [1.0; 4],
})?;
map.set(10, 8, Some(sand_tile))?;
let mut world = Bridge::new(map, Simulation::new(320, 192, 42))?;
world.set_conversion(sand_tile, Some(CellData::sand(0)))?;
let receipt = world.apply(Command {
    id: 1, expected_revision: None,
    edit: Edit::Break { x: 10, y: 8 },
});
world.step();
```

Bridgeは2つの可変世界を所有します。初期化には、全タイルの領域と同じセル寸法、空の外部コリジョン層、未使用のSimulationリクエスト列が必要です。既存粒子が初期固体タイルと重なるとエラーです。1セルを共通座標の1単位とし、1タイルは任意の複数セルを覆います。回転・異なる解像度の重ね合わせはM3では対象外です。

- **タイル → セル**：固体タイルの領域を独立した外部コリジョンとして登録します。粒子がタイル内部へ移動するのを防ぎます。
- **セル → タイル**：`occupancy(x,y)` で材料と占有を取得でき、`sand().contacts()` で隣接接触を問い合わせられます。固体の設置は、その領域に粒子が1つでもあると全体を拒否します。
- **破壊・粒子化**：`set_conversion(tile, Some(material))` をゲーム側で設定します。8×8タイルなら64個の粒子へ変換します。ルールなしの破壊はコリジョンを取り除くだけです。`None` で変換ルールを解除できます。
- **設置制約**：`set_placement_rule(fn(TileId, &[CellData]) -> bool)` は追加のゲーム制約です。固体設置時の粒子保護を無効化するものではありません。粒子を移動する処理は明示的な別操作として実装してください。
- **水や砂の制御**：`Edit::Paint` は登録した材料を配置します。変換ルールも任意の登録済み材料に設定できます。空気・不明な材料への粒子化は拒否します。

コマンドIDは単調増加です。古いIDはDuplicate、古いexpected_revisionはStaleとして拒否し、両世界を変更しません。設置・破壊は1タイル単位のトランザクションです。1つの要求を検証してからコリジョンとタイルを確定し、途中に物理stepを挟みません。複数タイルをまとめた一括トランザクションは提供しません。

`BridgePlugin::new(bridge, capacity)` はbounded queue、編集結果、読み取り専用Snapshotを提供します。計算workerに登録し、ゲームworkerから `BridgeClient::try_send`、`try_receipt`、`snapshot` を使います。キューがFullなら要求を保持して再試行します。結果を消費しない場合は次の編集を停止し、物理tickは続行します。終了後は要求チャネルを閉じます。

`Bridge::apply` を直接使う場合も、ゲームスレッドから可変世界を共有せず、計算laneでstep前に実行してください。`drain_changes()` は接続先が消費するまで保持し、少なくともtickごとに排出します。

## 検証と制約

テストは、重複破壊・古いrevision・粒子を覆う設置・ゲーム側制約・接触・初期衝突・寸法不一致・キューの逆圧・終了を対象にします。単体の物理テストと依存境界検査も維持します。

サンプルはgame worker144Hz、compute worker30〜60Hz、windowの描画要求60Hzです。指定したHzは目標周期で、端末の速度を保証するものではありません。GPU診断はLinux Xvfb + Vulkan llvmpipeで実行し、Windows 11実機の入力・表示確認とは分けて記録します。
