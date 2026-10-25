# M1：Core / Plugin の契約

## 依存方向

`charon-core` は標準ライブラリだけに依存します。Falling Sand、Tilemap、描画、ライティング、ウィンドウは含みません。

```text
ホストアプリ ──→ charon-core
     │
     └──→ 機能の Plugin ──→ charon-core
                  │
                  └──→ 独立した機能ライブラリ

接続用アダプター ──→ 接続する双方の公開インターフェース
```

M1のworkspaceは次の構成です。

| package | 役割 | 依存 |
| --- | --- | --- |
| `charon-core` | 最小実行基盤 | なし（stdのみ） |
| `sandengine` | 既存の独立した物理ライブラリ | rand / glam |
| `charon-m1-sample` | M1のゲームとサンプル専用の表示 | charon-core / minifb（任意） |

`sandengine` の Plugin 化はM2です。M1サンプルのソフトウェア表示・入力は、M3以降の正式な描画・入力APIの実装を意味しません。将来の機能はCoreへ直接追加せず、それぞれのライブラリと登録用Pluginとして作ります。

## ホストと実行順序

ホストが `App` を所有し、Pluginの登録後に `start()` を呼びます。Core自身はループ、時計、スリープ、ウィンドウを作りません。

1. 登録済みPluginの依存グラフを検証する。
2. 解決した順序で `init()` を呼ぶ。
3. ホストの要求に応じて `fixed_update()` / `frame_update()` を呼ぶ。
4. `shutdown()` で初期化の逆順に終了する。

`Plugin::name()` はアプリの生存期間中に変わらない、重複しない名前を返します。

- `dependencies()`：必須のPlugin。初期化・固定tick・フレームで先に実行する。
- `after()` / `before()`：相手が存在するときだけ適用する実行順序。存在を必須にしたい場合は `dependencies()` も使う。
- 無関係なPlugin同士は登録順を優先する。
- 依存不足・循環依存は、どの初期化処理も実行する前にエラーにする。
- M1は単一スレッドで順次実行する。Pluginの追加は開始前のみ。実行中の追加・削除・動的ロードは対象外。

順序の制約は全ステージ共通です。Plugin内ではステージごとのコールバックに処理を分けます。

## 時間の扱い

通常はホストが前フレームからの経過時間を `advance(elapsed)` に渡します。

- 既定は60Hz相当の固定tick、1フレーム最大8 tick。
- `with_timing()` で固定時間幅と最大tick数を変更できる。0はエラー。
- 固定tickを必要な回数実行した後、フレーム更新を1回実行する。
- 過剰な経過時間は蓄積上限で切り捨て、`FrameReport::dropped_time` で通知する。
- フレームの `Step::delta` はホストが渡した実際の経過時間。固定tickは設定した固定時間幅。
- `Step::index` は各ステージ独立の0始まりの番号。ゲームの更新回数として利用できる。
- `FrameReport::interpolation` は残りの固定時間の割合。描画補間の利用はホスト側で決める。

検証や外部ループへの組み込みでは `tick()` / `frame(delta)` を直接呼べます。これらは `advance()` の蓄積時間を変更しないため、通常の運用ではどちらか一方の駆動方式を選びます。

## リソースとイベント

`World` は型ごとのリソースとイベントを保持します。

- `insert<T>()` / `get<T>()` / `get_mut<T>()` / `remove<T>()` で型付きの状態を扱う。
- 1種類の型につきリソースは1個。同じ内部型を複数用途に使う場合はnewtypeを定義する。
- `send<T>()` でイベントを追加し、`events<T>()` で複数の読み手が同じイベントを確認する。
- 読み取りは消費しない。イベントの生成側が `clear_events<T>()` を呼び、保持期間を決める。
- 固定tickとフレームの間で自動消去しない。長期間蓄積させる場合のメモリ管理も生成側の責務。

M1サンプルでは、Game Plugin が固定tick冒頭で前tickのイベントを消去し、更新中に収集イベントを送ります。同じ固定tickで後続のScore Pluginが得点を更新します。View Pluginは毎フレーム得点リソースを読みます。この方式なら、1フレームに複数の固定tickがあってもイベントを取りこぼしません。

## 失敗と終了

状態は `Configuring → Running → Stopped`。コールバックが失敗した場合は `Failed` になります。

- 登録・依存検証エラーは `Configuring` のまま。依存を追加して再試行できる。
- 初期化失敗時は、失敗したPluginも含め、初期化を試みたPluginを逆順に終了する。部分初期化を片付けられる `shutdown()` を実装する。
- 更新失敗時も、初期化済みPluginをすべて逆順に終了する。
- 終了処理が失敗しても残りを続行し、元のエラーと終了エラーを返す。
- 明示的な `shutdown()` を推奨する。`Drop` は呼び忘れに対する終了処理の補助で、エラーを報告できない。
- 終了処理は一度だけ呼ぶ。停止・失敗後の更新や再起動はエラー。再起動には新しい `App` を作る。
- Pluginが所有する外部リソースや状態は、そのPluginの終了処理で解放する。残ったWorldの値はAppの破棄時にも解放される。

M1のエラー処理は `Result` による失敗が対象です。panicの隔離や、変更済みのWorldを自動的に巻き戻す機能は提供しません。

## 確認方法

```sh
cargo run -p charon-core --example empty
cargo test -p charon-core
cargo tree -p charon-core
python scripts/check_core_dependencies.py
cargo run -p charon-m1-sample --no-default-features -- --smoke-test
```

実機向けゲームは [M1サンプルの手順](../samples/m1-plugin-game/README.md)を参照してください。
