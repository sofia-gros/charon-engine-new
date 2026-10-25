# M4 Lighting API

照明は独立した光学データを入力にします。Tilemap、Falling Sand、Rendering、Coreのいずれも照明の必須依存にはしません。

| crate | 責務 |
| --- | --- |
| `charon_lighting` | 共通の座標・遮光・透過・発光データ、入力合成、光源、差分通知、CPU参照計算、照度取得 |
| `charon_tilemap_lighting` | Tilemapとゲーム側の光学設定を共通入力へ変換 |
| `charon_sand_lighting` | 粒子Snapshotとゲーム側の関数を共通入力へ変換 |
| `charon_lighting_gpu` | 独立したwgpu Compute Shaderによる照明計算と結果テクスチャ |
| `charon_lighting_render` | CPU結果の画像変換、汎用描画パスの選択、独立した合成Shader |

## 共通入力

`Geometry { width, height, origin, cell_size }` は照明専用の領域です。originはワールド座標、cell_sizeは1照明セルに対応するワールド単位です。カメラの表示範囲や物理のアクティブチャンクとは別に設定します。上限は8192×8192の各辺・合計1Mセルです。

`Optical` は次を持ちます。

- `opacity`：0〜1の遮光率。AOの形状入力にも使う
- `transmission`：RGBごとの透過率。水・色付きガラス・霧などを設定できる
- `emission_color` と `emission_strength`：発光色と強度。強度は0〜16

`Optical::AIR` は遮光せず、RGBを通します。`OPAQUE` は光を通しません。空気そのものを一律に明るくする処理ではなく、届いた光で照らします。

`OpticalGrid::raster_rect` はワールドの矩形を照明セルへ投影します。小さな壁を平均化して消さないよう、接触するセルを保守的に覆います。同じ入力レイヤー内の遮光はmax、透過はmin、発光はRGBごとのmaxです。照明セルより狭い隙間は閉じて見えることがあるため、必要な細さに合わせて解像度を選んでください。

複数レイヤーは同じorigin・cell_size・寸法へ変換してから `Scene::set_layer(id, grid)` で合成します。レイヤー間の透過率は積、遮光率は `1-(1-a)(1-b)`、発光はRGBごとのmaxです。登録IDの大小は重ね順を表しません。画像の描画レイヤーとも独立しています。透過的な背景は入力から外すなど、何を遮光体とするかはゲーム側で決めます。

## 光源と計算

```rust
use charon_lighting::{Geometry, Settings, Scene, PointLight, solve_cpu};

let geometry = Geometry {
    width: 80, height: 48, origin: [0.0, 0.0], cell_size: 4.0,
};
let mut scene = Scene::new(geometry, Settings::default())?;
scene.set_light(100, PointLight {
    position: [80.0, 100.0], color: [1.0, 0.75, 0.35], intensity: 1.4,
})?;
let field = solve_cpu(&scene.prepare())?;
let rgba = field.sample([90.0, 100.0]); // RGB照度と独立したAO
scene.remove_light(100);
```

空からの光は領域上端から各列へ入り、通過した材料の遮光・透過で減衰します。遮光体の表面セルは光を受け、その先への伝播を止めます。発光する固体は遮光を維持しながら周囲へ光を出せます。

M4の伝播は、4近傍のRGB最大値を有限回数伝えるセル方式です。距離減衰は `falloff_per_unit ^ cell_size`、伝播回数は `ceil(max_distance / cell_size)`、上限256回です。曲がった通路にも光が回り込みます。レイトレーシングや物理的なエネルギー保存を行うソルバーではありません。

`ambient_occlusion` は照明計算とは別に使用できます。周囲8セルの遮光率から接触部・隙間を暗くします。`ao_strength=0` で外せます。最終RGBは照明とambientをAOで減衰させ、発光源自身の明るさを確保して0〜1へ収めます。Bloomなどの演出は含みません。

## 編集・光源削除・更新領域

`set_layer` は光学データを比較し、実際に変わった範囲だけを変更通知に記録します。`TilemapInput::sync` は編集通知・mapのrevision・光学設定の変更を検知します。編集通知を取り逃がしてもrevisionの違いから再構築します。別のワールドへ同じ入力アダプターを使い回す場合は `invalidate()` を呼んでください。

粒子のアダプターはSnapshotから投影します。物理の「静止している」判定を照明の更新省略に流用しません。堆積した砂でも遮光し、火や独自の発光物質もゲーム側の変換関数で扱えます。

光源の移動・削除、レイヤーの削除、設定変更もsceneのrevisionを進めます。`dirty_region()` は伝播半径とAOの周辺領域を含み、`acknowledge(revision)` はその版だけを確認済みにします。古いrevisionの確認では新しい変更を消しません。

M4のバックエンドは、変更時に照明領域全体を新しい種から再計算します。前フレームの照度を初期値にしないので、光源の削除や通路の閉鎖で古い光が残りません。未変更ならホストまたはPluginがrevisionから計算を省略できます。領域内の部分更新は今後の最適化です。

カメラの外から届く光を扱うには、表示範囲に `max_distance + cell_size` 以上の余白を加え、そこにある地形・光源も入力します。照明領域の外部は暗く、AOでは開放空間として扱います。領域外の点光源は種を生成しません。上端にskylightを入れる契約なので、地下だけを切り出す場合はskyを0にするか、地表からの入力を別途維持してください。

## GPU計算と描画への合成

`GpuLighting::new(device, queue, geometry)` はウィンドウを必要としません。`headless` で単独検証用のコンテキストも作れます。`solve(&Prepared)` はCompute処理をsubmitし、GPU上の結果を `texture()` で公開します。live描画でCPUへのreadbackは行いません。`readback()` はCPU比較・取得・計測用の待機するAPIで、計算前の呼び出しはエラーです。

計算と合成は別Shaderです。

1. Compute Shaderが空の光・発光・点光源を種にし、ping-pongの伝播とAOを計算する
2. RGBA8 **linear** の照明テクスチャを出力する
3. Renderingの汎用 `bind_texture` で同じdeviceの画像を共有する
4. `register_sampled_pass` がgroup 2の画像を受け取り、描画Shaderで色へ乗算する

Rendering側は照明の内部構造を知りません。group 2は任意のサンプル画像です。合成Shaderには、照明領域全体のスクリーン矩形をvec4パラメータとして渡します。UIは別パスに残せます。CPU結果を画像へ変換する場合は `charon_lighting_render::image` と `GpuRenderer::upload_linear` を組み合わせ、sRGBの二重変換を避けます。

GPUリソースは同じdevice/queueから作成してください。外部から共有したテクスチャへのCPU上書きは拒否します。解像度を変更するときはGpuLightingを再生成し、共有テクスチャも差し替えます。

## Plugin・周期・品質

Coreを使う場合は `charon_lighting` の `plugin` featureで `LightingPlugin` と `LightingState` を利用できます。入力をsceneへ反映するPluginをlightingより前に並べ、独立した計算workerへ登録します。CPU参照バックエンドはrevisionが変わったときだけ更新し、shutdownでリソースを解放します。

GPUの場合も、照明入力とComputeの提出は描画パスから独立しています。サンプルはゲーム144Hz・物理計算30/60Hz・描画要求60Hzに加え、照明提出30Hzを標準とします。描画は直近の完了結果を使用します。GPUの提出はウィンドウスレッドで行いますが、更新周期は描画回数と分けています。

解像度、最大距離、距離減衰、AO、ambient、sky、更新頻度を設定できます。サンプルでは `--light-cell-size 1|2|4|8`、`--lighting-hz 10..60` を指定できます。周期は目標値であり、全ての端末・品質でのfps保証ではありません。

[数値・画像・GPU時間の検証記録](./performance/m4-lighting.md) / [Cave Lightの操作方法](../samples/m4-cave-light/README.md)
