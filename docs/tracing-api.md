# M6.1：独立したレイ／パストレーシング照明

PCではRTX（VulkanのハードウェアRay Query）、Path、M5までの既存照明を選べます。MobileではLite Pathと既存照明を使います。2Dの光学グリッドを入力とし、TilemapとFalling Sandの両方を遮光・透過・発光の対象にします。

## APIと利用側の責務

Core／照明プラグインは設定ファイルの読込・保存、デモの画面設定、ユーザーの設定値、既定方式やフォールバックの選択を持ちません。JSON、メニュー、無効項目の表示、設定を変える判断は [デモ側](../samples/rt-light-lab/src/config.rs) と [利用側のUI](../samples/rt-light-lab/src/settings.rs) に置いています。

- `LightingDevice::info()`：そのdeviceで実際に有効な `rtx / path / lite_path / compute / pc / mobile` を取得。
- `LightingDevice::supports(mode, platform)`：利用可能ならtrue、未対応ならfalse。
- `TracingSlot::try_switch(device, request, scene).await`：未対応なら `Ok(false)` を返して現在の方式を維持。対応時に候補を準備して成功後に切り替え、`Ok(true)` を返す。
- 候補の生成・Shader初期化・入力検証の失敗はErrを返し、現在の方式を維持する。
- `Mode::Legacy` への切り替えはトレーサーを解放する。別の既存照明プラグインと描画への接続は利用側が担当する。

利用側の概念例：

```rust,ignore
// 自分のゲームが持つ処理。Core／PluginのAPIではない。
let cnf = my_game::load_cnf()?;
let dev = LightingDevice::new(device.clone(), queue.clone())
    .with_adapter(adapter.features(), &adapter.get_info());
// 起動時の能力確認。device再生成時には新しいLightingDeviceでやり直す。
let _rtx_ready = dev.probe_rtx().await?;
let info = dev.info();
let requested = my_game::choose_mode(&cnf, &info);
// 実際のGeometry・品質・光学スナップショットはゲームから渡す。
let accepted = slot.try_switch(&dev, request_for(requested), optical_scene).await?;
if !accepted {
    my_game::reject_setting_change();
}
```

`SwitchRequest`は呼び出しに必要なmode・platform・計算予算・周期だけを持つ値です。ファイル形式や保存先、UIの選択状態を持ちません。Coreを使わない場合は `GpuTrace` と `LightingDevice` を直接使えます。

## モジュールの境界

| 部品 | 責務 |
| --- | --- |
| charon_tracing | 共通入力、反射率、CPU参照、可視性、履歴。依存はcharon_lightingだけ |
| charon_tracing_gpu | GPU計算と結果texture、デバイス能力、切り替え。rtx featureは任意 |
| charon_tracing_gpu / plugin feature | SceneのWorldにTracingSlotを所有し、終了時に解放するTracingPlugin |
| charon_lighting_render | 既存の照明合成Shader。トレース結果も同じ方式で合成 |
| charon-rt-sample | ファイル・UI・選択ルール・保存・無効表示と、既存照明への接続 |

`charon_engine` の `tracing` featureはCPU/APIだけ、`tracing-gpu` はGPUとPlugin、`rtx` は追加でハードウェアRT経路を公開します。Core・Tilemap・物理・既存描画がトレーサーへ依存することはありません。

## TilemapとFalling Sand

入力はM4/M5と同じ光学Sceneの `prepare()` です。タイルの壁・発光鉱石と、砂・水・火などのスナップショットを別アダプターで共通レイヤーへ変換し、合成した遮光・透過・発光をトレーサーへ渡します。

RTX用の加速構造もこの合成入力の不透明境界から作るため、対象はTilemapだけではありません。粒子が移動して不透明セルの形が変わったら境界を再構築します。水など半透明部分はグリッドの透過計算を併用します。光源だけの移動ではBVHを再構築しません。

物質の反射率は `TraceScene.reflectance` のセル別RGB（0〜0.95）で渡せます。プラグインが砂や石の反射率を固定しません。既定値は0.65です。Tilemapだけでも、粒子だけでも、同じ光学入力から利用できます。

## 計算方式と予算

2Dグリッド上でサンプルごとに方向を選び、可視性レイと拡散反射の経路を追います。光源への直接サンプリング、物質の透過、距離減衰、空の光、発光、反射、AOを扱います。3DのPBR材質・メッシュレンダラーではありません。

| モード | 既定の新規samples/update | 反射上限 | 探索上限 | 蓄積上限 |
| --- | --- | --- | --- | --- |
| PC Path / RTX | 4 | 2 | 192セル | 256 samples |
| Mobile Lite | 1 | 1 | 64セル | 32 samples |

Liteは最大2 samples、1反射、96セル、64蓄積samplesに制限します。デモではLiteを8 world units/セルに落とし、Path/RTXは既存光学Sceneの2〜4 units/セルを使います。これらの品質選択はデモ側のルールです。

毎回の計算は有限です。探索予算が尽きたレイを「遮るものなし」と扱いません。蓄積は有限の正確な平均で、地形・水・火・光源・光学設定・反射率・解像度を変えた場合は旧履歴を破棄します。Sceneを入れ替えて同じrevisionになった場合も入力本体を比較します。

静止している場面は蓄積上限で計算を止めます。移動時も直接光は決定的に評価し、間接光だけに遮蔽を考慮した空間フィルターを適用します。動的な世界の時間再投影は未実装で、間接光の残留ノイズはあります。

GPU結果はtextureとして描画へ渡し、通常描画ではCPU readbackしません。明示的なreadbackではLightFieldとworld座標のsample APIを利用できます。

## RTXの対応条件と検証分担

wgpu 26の `EXPERIMENTAL_RAY_QUERY` と `EXPERIMENTAL_RAY_TRACING_ACCELERATION_STRUCTURE` を使うVulkan経路です。DXRバックエンドではありません。対応ドライバーと、RT機能を有効にして生成したdeviceが必要です。GPUの製品名だけで対応判定しません。

不透明境界をz方向へ押し出した三角形のBLAS/TLASを構築し、ShaderからハードウェアRay Queryを実行します。262144境界面を上限とし、超過はエラーにします。実験的APIであり、地形変更時のBVH再構築コストもあります。

RTX実機の表示・遮光・透過・動的更新・切り替え検証はユーザーが担当します。こちらではRustのコンパイル、RT用WGSLの静的検証、境界メッシュと対応判定を確認します。Path/LiteのGPU実行結果を、RTX実機での実行結果として扱いません。

Mobile用Liteのデスクトッププレビューと、Android/iOS実機の検証も区別します。APK・iOSアプリ、タッチ用ホストは今回の成果物に含まれません。

## 再起動・デバイス再生成と真偽値

デバイス作成時の機能要求と加速構造limitsについては、[紹介されたwgpu v28の記事](https://zenn.dev/kokutoupan/articles/eefc517ac4210d)も参考にしています。本実装のwgpu 26では対応する機能2種と `using_minimum_supported_acceleration_structure_values()` を使います。v28専用のExperimentalFeaturesのAPIを、そのままv26へ持ち込みません。

`LightingDevice::info()`は、選択中アダプターの対応情報（任意）、現在のdeviceでの有効化、実行検証済みの状態を分けます。

- `rtx_adapter`：そのアダプター／バックエンドがRT機能を報告するか。情報がなければNone。
- `rtx_enabled`：現在のdeviceに必要なfeaturesとBLAS/TLASのlimitsが有効か。
- `rtx_requires_recreation`：対応アダプターだが現在のdeviceにRTが有効でない状態。
- `rtx`：有効化だけでなく、小さな実ハードウェアRay Queryのprobeにも成功した場合のみtrue。

`probe_rtx().await` は、既知の壁をBLAS/TLASに構築し、別の検証用entry pointで実際にRay Queryを実行してヒット位置とセルを読み戻します。DDAの代替結果でprobeを成功扱いにしません。deviceを生成し直すと、この確認済み状態も未確認からやり直します。CPU／ソフトウェアアダプターをハードウェアRTXとして表示しません。

`try_switch` は必要なprobe、候補初期化、最初のdispatchとreadbackの確認後にtrueを返します。未対応ならfalseで旧方式を維持し、初期化・実行の失敗はErrで旧方式を維持します。「ファイルへRTX=trueと保存できた」ことを、適用成功とは扱いません。

デモの `enable_rtx_device:false` で起動すると、RT対応GPUでも現在のdeviceではRTXが無効です。この場合は「再起動が必要・未適用」と表示します。UIからの要求は `pending` に保存し、現在の `lighting` は変更しません。Sで保存してプロセスを再起動した後も、新deviceのprobeと実適用が成功するまではRTXを有効と表示しません。失敗時は既存照明を保持し、未適用の理由を表示します。

再起動要求・ファイル保存・新deviceの作成・アプリを再起動する操作はデモ／ゲーム側の責務です。Pluginは自動再起動や設定ファイルの書き換えをしません。

## 動く光源と計算スレッド

直接光は、光源ごとの遮蔽・透過レイと距離減衰を毎回決定的に評価します。間接光だけを確率的に積算し、壁をまたがない空間フィルターを適用します。光源の移動で履歴が無効になっても直接光に低サンプルのランダム模様が現れません。光源座標は `TraceScene.point_lights` にワールド座標のまま渡せるため、セル単位の位置丸めも不要です。発光セルは空間的に集約し、明示的な点光源と合わせて最大64個で計算量を制限します。間接光の残留ノイズや、低解像度グリッドの形状誤差までなくなる保証はありません。

`TracingWorker::spawn(runtime, hz, source)` は専用スレッドで入力取得・CPU準備・GPUコマンド生成を実行します。デモのPC設定は144Hz、ゲーム144Hz・描画60Hz・物理30〜60Hzは独立しています。144Hzは投入上限で、GPUの完了を確認するまで次の照明ジョブを投入しません。遅いGPUに未処理フレームを積み上げず、次の処理時に最新スナップショットを取得します。描画側は結果textureを参照します。モードの初回生成・検証と明示的なreadbackには待機が発生します。

`charon_tasks::ThreadBudget::detected()` は利用可能な論理CPU数を取得し、ホスト・ゲーム用の2枠を除いた重い処理を重みで配分します。8 CPUは物理3・照明3、4 CPUは物理1・照明1です。各所有スレッドもジョブを実行し、追加のCPUジョブは同じ上限付き `TaskPool` を共有します。光学レイヤーの生成・光学データ/AOのパックと、互いに影響しない物理領域を並列処理します。物理は水平移動・安息角の参照範囲を含む列領域に分け、結合した領域や独自更新関数がある場合は従来の逐次更新に戻します。接続した砂山を無理に分割して更新順や衝突を壊すことはありません。

CPU数に応じた上限であり、全コアを常時100%にする仕組みではありません。GPUのレイ演算自体はCPUスレッドの追加では高速化しません。デモ側の `--cpu-budget 4` で追加ワーカーなしの構成を確認できます。設定ファイル・UI・スレッド予算の選択ポリシーは引き続きデモ／ゲーム側が持ちます。
