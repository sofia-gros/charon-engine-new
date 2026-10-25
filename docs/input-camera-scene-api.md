# M5：Input / Camera / Scene

入力、カメラの数学、Sceneの管理は別々のライブラリです。どれも標準構成ではstdだけに依存し、物理・Tilemap・Rendering・Lighting・ウィンドウを必須にしません。

| crate | 役割 | 任意の接続 |
| --- | --- | --- |
| `charon_input` | ボタン状態、押下・解放・リピート、ポインター、ホイール、フォーカス、Scene境界の入力フィルター | `plugin` featureのInputPlugin / InputClient |
| `charon_camera` | 移動・ズーム・表示領域・座標変換と逆変換 | `plugin` featureのCameraPlugin |
| `charon_scene` | factory、開始・停止・破棄、切り替え、所有権、世代付きSceneId | `plugin` featureのScopedScene / ScenesPlugin |
| `charon_input_winit` | winitのイベントから入力データへの変換 | OSアダプターだけを選べる |
| `charon_camera_render` | 汎用Quadの投影と、描画パスごとのviewport clip | Tilemap・物理・照明の型を参照しない |

## 入力

`InputState::begin_tick()` で一時的な状態をクリアし、`apply(InputEvent)` でそのtickのイベントを取り込みます。`held`、`pressed`、`released`、`repeated` は別々の状態です。短い押下と解放が同じtickに届いても両方のedgeを記録します。OSのリピートは新しい押下にはしません。

`Button::Key(Key(u32))` と `Button::Mouse(u32)` は別の名前空間です。Keyは開いたID型で、ゲーム側でアクションやキー割り当てを決められます。winitアダプターは英数字を大文字ASCIIへ、矢印などを `keys` の定数へ、その他の物理キーを専用範囲へ変換します。論理文字列やIMEではなく、物理ボタンのAPIです。

PointerとResizeは物理ピクセルを使います。Wheelは行単位で、winitのpixel deltaは48pxを1行として変換します。非有限の入力はエラーです。ホイールの蓄積は有限範囲に収め、次tickでクリアします。

フォーカスを失うと、heldと押下・リピート、ポインターとホイールをクリアし、解放を記録します。フォーカス外の押下と、未知のheldに対するRepeatでは入力を再開しません。

Coreへ接続する場合は `InputPlugin::new(capacity)` がPluginとInputClientを返します。OS側の `try_send` はbounded queueで、Fullならイベントを保持して順序どおり再試行します。入力Pluginをゲームロジックより前に登録し、そのfixed tickで状態を読みます。最後の送信口が切断された場合も入力を解除します。shutdownは受信口を閉じ、公開snapshotを解除状態に戻します。

Scene切り替え時は、ホストの `InputBarrier::scene_changed()` を使います。前のSceneから物理的に押し続けられているボタンは、一度解放するまで次のSceneへ渡しません。イベントの再試行キューにもSceneIdを付け、旧Scene宛ての要求を新しい送信口へ流さないでください。

## カメラ

`Camera2D` はcenter（ワールド座標）、zoom（物理pixel / world unit）、viewport（物理pixelの矩形）を管理します。回転はM5には含めません。

```rust
use charon_camera::{Camera2D, Rect};

let mut camera = Camera2D::new(
    [160.0, 96.0], 3.0,
    Rect { x: 0.0, y: 72.0, width: 960.0, height: 576.0 },
)?;
camera.pan([20.0, 0.0])?;
camera.zoom_at(4.0, [480.0, 360.0])?;
let screen = camera.world_to_screen([108.0, 84.0])?;
let world = camera.pick(screen); // viewport外ならNone
```

`zoom_at` は指定pixelの下にあるワールド座標を保ちます。`set_zoom_limits` は許容範囲、`fit(world_rect)` は全体表示です。リサイズ時の `set_viewport` はcenterとzoomを維持します。値が非有限になる操作はエラーにし、複合操作の途中状態を残しません。

`world_to_screen` / `screen_to_world` は任意の有限点の変換です。`pick` はviewport内だけを受け付けるため、UI部分を誤って掘るのを防げます。矩形のwidth・heightは正にし、反転は描画側のUVで扱ってください。

CameraPluginはWorldへCamera2Dを配置する接続用Pluginです。更新の所有者を1つにし、他のスレッドへはコピーのsnapshotを公開します。サンプルではゲームworkerが144Hzでカメラと入力を更新します。

## 地形・Sprite・粒子・照明との整合

描画用のworld座標Quadを作った後、`charon_camera_render::apply` で同じCameraを適用します。指定したパスだけを変換し、UIは対象から外せます。不正な矩形がある場合は、途中まで変換したframeを残しません。

`DrawFrame::clips` はパスごとのscissor矩形です。描画バックエンドはsurfaceの範囲へclampし、cameraの外へ拡大した地形がヘッダーやフッターを上書きするのを防ぎます。指定しないパスはsurface全体です。

照明はワールドに固定したデータのまま使えます。照明領域を `camera.project_rect` でscreen矩形へ変換し、合成パスのパラメータに渡します。地形と同じCameraを1frameにつき1つのsnapshotとして使うことで、移動・ズーム・リサイズ後も照明のUVが一致します。

クリックは `camera.pick(pointer)` でworldへ戻し、Tilemapや物理のAPIへ渡します。CameraやInput自体は、タイル寸法・粒子の型を知りません。

## Sceneと所有権

標準のSceneManagerはCoreに依存せず、`Scene` を実装したオブジェクトを管理します。factoryで新しいインスタンスを作り、start、fixed_update、frame_update、stop、destroyを呼びます。

- SceneIdはmanagerの識別子と世代を持ち、再開始や失敗した候補のIDを再利用しません。
- 現在のSceneが返したDirective、または `request_switch(from_id, target)` を境界で確定します。古いSceneや別managerのIDは拒否します。
- 切り替え先の名前を検証し、候補のstartを完了してから旧Sceneを停止・破棄します。候補の作成・開始に失敗した場合は候補を掃除し、現在のSceneを維持します。
- 旧Sceneのstopが失敗してもdestroyを続けます。cleanupのエラーはSceneEventへ記録します。ホストはイベントを消費して、表示や終了などの方針を決めます。
- 切り替えを要求したfixed更新の後、旧Sceneの残りのcatch-up時間を新しいSceneへ持ち越しません。
- shutdownは冪等です。stop/destroyは部分的なstartにも対応してください。SceneEventは最大256件で、あふれた件数を別途取得できます。

標準は同時に1つのactive Sceneです。pause/resumeや複数Sceneの重ね合わせは今後の拡張です。候補の準備中は一時的に2つの所有スコープが存在するため、GPU IDや状態を共有して上書きしないでください。

### CoreのPluginをSceneに所属させる

`charon_scene` の `plugin` featureにあるScopedSceneは、Sceneごとに新しいApp / Worldを所有します。SceneLogicのstart前にPluginを初期化し、fixed tick後にSceneLogicの更新を呼びます。SceneManagerと子Appのfixed deltaは一致させます。

終了順は次のとおりです。

1. SceneLogic.stopで子workerを停止し、joinする
2. 子AppのPluginを逆順でshutdownする
3. SceneLogic.destroyで残りのScene所有リソースを取り除く
4. SceneのオブジェクトとWorldをdropする

SceneLogicの状態やWorldへ、workerのハンドル・送信口・アセットハンドル・GPU IDなどを所属させます。stopは初期化途中でも、他のリソースの解放を続けてください。Rendererのsurface/deviceなど、ホストが明示的に所有するものはSceneの外に残せます。

`ScenesPlugin` はSceneManagerを親AppのWorldへ接続する任意のアダプターです。低レベルのSceneManagerだけを直接駆動しても構いません。Sceneのコールバックはホストスレッドで実行し、内部のworkerは各自のApp / Worldを所有します。

### アセットとGPU

アセットのストアはSceneごとに生成します。旧ストアのHandleは別ストアを参照できません。外部が明示的に保持したArcや読み取り専用の履歴まで強制的に破棄するものではありませんが、新しいSceneの可変状態として再利用しません。

GpuRendererの `allocate_texture_id` / `allocate_pass_id` は登録済みのIDを避け、割り当てたIDを再利用しません。候補Sceneの準備時に旧Sceneのテクスチャやパスを上書きせず、所有したIDだけを `remove_pass` / `remove_texture` で解放します。0の白テクスチャと標準パスはホストに残します。

GPUに提出済みのコマンドの参照はwgpuが完了まで保持します。解放確認では、Scene所有の公開ハンドルがなくなったことと、診断用のGPU待機後のリソース数を確認します。

## 検証

Inputのtap/repeat/focus/disconnect、Scene境界のheld抑制、Cameraの逆変換・zoom anchor・resize、照明UVと複数の描画パスの整合、候補の失敗・古いID・cleanupエラーをテストします。

Scene Labはゲーム144Hz、計算30/60Hz、描画要求60Hzを維持します。Tilemap単体・Falling Sand連携・Lightingなしの構成を検証し、実ウィンドウでは移動・ズーム後の掘削、Scene再開始、CPUアセットの解放、メニュー復帰時のGPUリソース数を確認します。Linux/Xvfbでの検証と、Windows 11実機確認は区別します。

[サンプルの起動・操作](../samples/m5-scene-lab/README.md)
