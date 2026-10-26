# Ray / Path lighting

ゲームと物理は2Dのままです。PCのPath/RTXはセルを描画用の薄いprismとして扱い、法線、拡散反射、鏡面反射、屈折、吸収、自己発光を計算します。Liteは従来の2D拡散照明、M5は既存の照明プラグインを使います。汎用の3Dメッシュレンダラーや多重散乱・集光模様のソルバーではありません。

## 入力と材質

`TraceScene::new(Prepared)` は従来の光学入力から既定材質を作ります。`reflectance` は引き続きセル別の拡散反射率です。`materials` には `Material` のセル別配列を渡せます。

- `diffuse`：二次反射に使う線形RGB。表示面のベースカラーは描画側の画像から取得します。
- `roughness / specular / ior`：粗さ、垂直入射の鏡面反射率、屈折率。水はFresnel反射とSnellの法則、全反射を使います。
- `absorption`：world距離あたりのRGB吸収係数。`exp(-sigma * distance)` で積分します。従来の `Optical.transmission/opacity` もworld距離に換算して組み合わせます。同じ吸収を両方に重複設定しないでください。
- `depth / normal`：prismの半厚みと表示面の単位法線。厚みはworld単位で、セル解像度には依存しません。水面や砂の微細な法線は利用側が生成します。
- `emission`：線形HDRの自己発光。同じデータを表示・照明・反射に使います。`radiance(index)` は明示的な光源と材質発光の最大値を返します。
- `dielectric / dynamic`：透過する境界を持つ材質と、動きうる材質の指定。材質名や物理のIDはPluginに固定しません。

prismは正面・背面・側面を持ちます。同じ媒質の隣接セルに内部面は作りません。厚みの違う隣接セルでは露出する部分だけを残します。Pathはソフトウェアのグリッド／slab探索、RTXは同じ形状の三角形をハードウェアRay Queryで探索します。

発光セルは面積と光束に応じた確率分布（CDF）でサンプリングします。小さな強い光源が低確率で選ばれた時だけ過大な寄与を返す問題を抑えます。明示的なPointLightは旧APIの強度から固定の光束へ換算し、2 world units四方の表示用光源として扱います。これらの光源サイズと吸収距離は画面のズームで変わりません。レイ探索の予算が尽きた場合は遮光扱いにします。

`TraceScene::at_resolution` は従来の保守的な光学グリッド再標本化です。粗いグリッドに取り込んだ粒子の隙間を復元できません。高品質の入力は元のタイル・粒子から必要な解像度で直接作ってください。RTデモは両PC方式で1粒子=1光学セルにします。灰を半透明にして補正する処理はありません。

## HDR出力と描画

Path/RTXの `GpuTrace::texture()` は `Rgba16Float` の横4面atlasです。左から拡散照明、反射・透過、自己発光、法線・形状guideです。従来の単一照明画像とは異なるため、`charon_lighting_render::HDR_SHADER` で合成してください。Liteは従来のRGBA8照明画像と `SHADER` を使います。

通常描画はGPU textureを共有し、CPU readbackしません。

- `readback_hdr()`：Path/RTXの未トーンマップ・未空間フィルターの `HdrField`。`diffuse / specular / emission` を別々に保持し、1を超える値を返します。
- `readback()`：既存の0〜1 `LightField` へ変換する互換用の数値取得。HDRの検証には `readback_hdr()` を使います。
- `charon_tracing::hdr::Accumulator`：同じprism、材質、乱数系列、履歴を使うCPU参照。従来の `Accumulator` はLite用の2D参照として残します。

`GpuRenderer::set_hdr(Some(HdrSettings))` は指定したworld layerまでをRGBA16Floatの中間画像に描画します。合成は `base_color * diffuse + specular + emission` です。発光を元画像の暗い色で乗算しません。輝度閾値を超えた成分だけを1/4解像度の分離フィルターでブルーム処理し、固定露出とACES近似でトーンマップします。sRGB出力で表示色変換を1回だけ行い、UIはその後に描画します。`set_hdr(None)` は中間画像を解放して従来の経路に戻します。

`HdrPost` は同じ後処理をオフスクリーン描画から使うための部品です。描画サイズの変更時は作り直します。設定画面、JSON、露出の選択、モード切り替えの方針は利用側が持ちます。

## 品質・履歴・性能

| 方式 | samples/update | 拡散反射 | 探索セル | 静止時の蓄積上限 | 反射 / 透過予算 | 空間フィルター半径 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| RTX | 16 | 4 | 512 | 4096 | 4 / 8 | 2 |
| Path | 4 | 2 | 192 | 256 | 1 / 4 | 1 |
| Lite | 1 | 1 | 64 | 32 | 従来方式 | なし |

`Quality` は従来のサンプル・拡散反射予算、`TraceScene.surface_quality` は `SurfaceQuality` の反射・透過・デノイズ予算です。RTX用のSceneには `SurfaceQuality::for_mode(Mode::Rtx)` を設定してください。高品質設定を負荷によって自動で下げる処理はありません。

間接拡散光を履歴に蓄積します。直接光と反射・透過は最新入力から決定的に評価し、静止した水の表示を乱数の変化に依存させません。少数の光源は全て評価し、多数の場合は光束CDFを層化してPathは8、RTXは64の光源サンプルで積分します。点光源の面積と粗い鏡面ローブには固定の2×2 quadratureを使います。

滑らかな水では反射と屈折を確率で二者択一にせず、Fresnelの重みで両経路を追跡します。反射先の不透明面は直接光・ambient・粗い鏡面成分で評価し、その先の拡散多重反射を再サンプリングしません。このため反射先の間接色移りは近似になります。鋭い鏡面成分を狭いカーネルでぼかしてノイズを隠す処理も行いません。

変更セルからレイのworld距離上限までを保守的に無効化します。この範囲には光源、遮蔽、材質、法線、奥行きの変更を含み、影響のない領域は残します。world座標に履歴を持つためカメラ移動・ズームで再投影誤差は発生しません。原点・解像度や全体設定を変える場合は全履歴を破棄します。

動的材質であっても静止した入力は蓄積を進めます。変化した受光セルと静的な壁の編集による影響範囲は最新サンプルから作り直します。動的粒子だけが変化した場合、周囲の間接光は `dynamic_history` update分の短い履歴に縮めて更新します。直接光・反射・透過は即時更新ですが、間接光にはこの短い応答遅れがあり、完全に履歴を消す方式よりノイズを抑える近似です。`dynamic_history` は変更時の動的材質の短い履歴予算（update単位）です。発光は時間・空間フィルターを通しません。

間接拡散光のまれな過大サンプルには `SurfaceQuality.indirect_clamp` を適用します（Path 2、RTX 4、0で無効）。現在の直接光とsky/ambientから決めた相対上限を超える単一サンプルを、RGB比を保って制限します。これは低サンプル時のfireflyを減らすためのバイアスを伴う近似です。自己発光・直接光・反射のHDR値を0〜1に切り詰める処理ではありません。デノイズと拡大補間は法線、奥行き、材質、途中の遮蔽物を確認し、壁越しや角をまたぐ混色を抑えます。

`GpuTrace::metrics()` はCPU submit時間、CPUの形状準備時間、バッファ／textureの概算bytes、面数、可能なdeviceではGPU trace+resolve時間を返します。GPU timestampのreadbackは非同期です。`gpu_ms=None` は未取得または非対応で、CPU時間をGPU時間として代用しません。ドライバー内のBVHメモリと表示・後処理時間はこのGPU時間／bytesに含みません。

## 検証

```sh
cargo test -p charon_tracing -p charon_tracing_gpu --features charon_tracing_gpu/rtx,charon_tracing_gpu/plugin
cargo test -p charon-rt-sample -p charon_render
cargo check -p charon-rt-sample --no-default-features
cargo run -p charon-tracing-check
cargo run -p charon-tracing-check -- --gallery target/lighting-gallery --1080p
```

Galleryは固定カメラ・固定露出で、拡散のみ（stage-0）、HDR発光（stage-1）、反射・透過を追加（stage-2）と、炎と不透明粒子が動く24フレームを保存します。過去バージョンのスクリーンショットではなく、現在の同じシーンで効果を比較するための画像です。実際の合成・ブルーム・トーンマップのシェーダーをGPUで実行します。`--static-only` は静止画だけ、`--rtx` は実ハードウェアのRTX経路を要求します。非対応GPUをRTXの検証成功として扱いません。

```sh
ffmpeg -framerate 12 -i target/lighting-gallery/motion-%03d.png -c:v libx264 -pix_fmt yuv420p target/lighting-gallery/motion.mp4
```

RTXはwgpu 26の実験的Vulkan Ray QueryとAcceleration Structure機能を使います。対応GPUだけでなく、機能・limitsを有効にしたdevice、実Ray Queryの起動時probe、候補の最初のdispatch成功が必要です。未対応は `Ok(false)`、初期化・検証エラーは `Err` で現在の方式を保持します。再起動要求や保存はデモ側の責務です。`LightingDevice::info / supports` と `TracingSlot::try_switch` の意味は従来と同じです。

通常の開発環境ではCPU参照、Path/Lite GPU実行、RTXシェーダーと境界meshを検証できます。RTX実機の画質・動的BVH更新・性能は別途実機で確認します。ソフトウェアVulkanの結果はRTXの性能測定ではありません。

## 低サンプル時の反射の回帰確認

```sh
cargo run -p charon-tracing-check -- --gallery target/lighting-noise-fix --1080p --noise-check
```

固定入力を異なる乱数フレームで描き、反射・透過のHDR成分にフレーム間差が生じないことを確認します。`low-samples-0.png` と `low-samples-1.png` は8 samples/updateの最初の2フレームです。直接光・反射に対するこの検査と、依然として確率推定を使う間接拡散光の収束は区別します。

### 実時間スケジューリング

`Tracing::submit` は最新スナップショットを置き換えるだけです。計算は設定Hzの `tick` で行い、前のGPU更新が未完了なら次の投入を保留します。モード切替の初回検証だけは明示的に実行・readbackします。`GpuTrace::render` はオフライン用途の明示的な投入を引き続き許可し、`is_idle` でノンブロッキングに完了を確認できます。

HDRの重いdispatchは8行ずつの独立したsubmissionに分けます。これはGPU watchdogを避けるための実行単位の制限であり、RTX実機でのクラッシュ解消やFPSを保証するものではありません。総サンプル数・形状解像度は維持します。入力が変わらない一次照明・表面反射はキャッシュし、粒子の変更影響範囲は再評価します。空の直接照明は固定quadratureに分離し、Liteも空・直接光を固定積分します。二次拡散輸送は引き続き確率的です。
