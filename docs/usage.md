# 導入と使い方

## 必要な環境

- Rust stable、Git。
- Windows 11: MSVC版Rust用のMicrosoft製リンカーとWindows SDK（Visual Studio Build Toolsから導入可能）。既にRustのexeをビルドできれば追加導入は不要。エンジンはRust、shaderはWGSLで記述する。
- Linux: Cリンカ、X11、VulkanまたはOpenGL ESドライバ。ウィンドウ不要のCPU検証も可能。
- `rust-toolchain.toml` はstableを指定する。更新は `rustup update stable`。依存バージョンはCargo.lockで管理する。

```console
git clone https://github.com/sofia-gros/charon-engine-new.git
cd charon-engine-new
```

開発ブランチを確認する場合は、そのブランチをcheckoutしてから起動する。

## Sandbox

[Linux操作記録（GIF）](media/sandbox-linux.gif)

壁を描く→砂を積もらせる→壁に穴を開ける→リセットする、実際のマウス/キー入力とウィンドウ描画の記録。Linuxの仮想画面とソフトウェアGPUで約14秒収録。フルHDの画面を動画では960×540に縮小し、8fpsを目安にキャプチャしている。動画のfpsはエンジンの性能値ではない。

![Sandbox on Linux](media/sandbox-linux.gif)

WindowsのPowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/run.ps1
```

このプロセスだけローカルスクリプトを実行し、恒久的な実行ポリシーは変更しない。通常の開発中は `-DebugBuild`、比較時は `-Workers 1` / `-Workers 4`、密度変更は `-Grid 960x540` を指定できる。既定はrelease、4ワーカー、480×270セル。

Linux、または直接Cargoで起動:

```console
cargo run --locked --release -p charon-sandbox -- --workers 4 --record artifacts/local/metrics.csv
```

クラウドでは先に `source scripts/env.sh` を実行する。初回/依存更新時のみ `bash scripts/setup.sh` を使用する。

ウィンドウの初期サイズは1920×1080。左ドラッグで砂、右で消去、中で壁、Rで初期化、Escで終了。表示サイズとセル数は独立している。端の壁と床は消去できない。

タイトルに描画FPS・sim Hz・tick処理時間・使用/設定ワーカー数・砂の数・フレーム間隔p99を表示する。休止時は使用ワーカー0になる。描画は最大144 FPSを狙い、シミュレーションは別スレッドの固定60 Hzで進む。

PowerShell起動では `artifacts/local/` に時刻付きCSVとRustバージョンを保存する。通常モードのCSVは1秒区間の値で、tick処理時間は直近tickの値。更新/画像生成の時間は直近tick、upload_enqueue_ms・upload_bytes・acquire_ms・encode_present_msは直近描画のCPU側呼び出し時間と転送要求量。converted_cellsは直近tickで画像へ変換したセル数。GPU実行時間や物理的な画面提示はまだ測定していない。VSyncやOSのスケジューリングも描画FPSへ影響する。

## Platformer

```console
cargo run --locked --release -p charon-sandbox -- --play --workers 4 --record artifacts/local/platformer.csv
```

Windowsは `scripts/run.ps1 -Play` でも起動できる。既定は480×270セル。A/Dまたは左右キーで移動、Spaceでジャンプ、Fを押している間は足元を掘る。Bで砂の当たり判定を切り替える。壁は常に足場になる。マウスの砂/壁/消去も使える。Rで地形・プレイヤー・判定設定を初期化する。

[Linux操作記録（GIF）](media/platformer-linux.gif)

物質は砂plugin、移動は砂に依存しないplatformer plugin、両者をつなぐ判定方針はサンプル側が所有する。キャラクターは地形と別の矩形としてGPU描画し、地形画像へ焼き込まない。`upload_bytes` は地形、`overlay_upload_bytes` は矩形頂点の転送量。操作モードのCSVは約0.1秒ごとに位置・着地・埋まり・砂の判定設定も記録する。

まず軸に沿った矩形とセルの衝突を扱う。薄い床の通り抜けを防ぎ、支えが消えると落下する。回転・坂・セルを押しのける双方向の物理は未実装。新しい地形に埋まった場合は16セル以内の空きへ移し、空きがなければ赤く表示して移動を停止する。周囲の消去かRで戻せる。`python3 scripts/capture-linux.py --sample platformer` で操作検証と録画を再実行できる。

操作を再現するための入力記録:

```console
cargo run --locked -p charon-sandbox -- --play --input-log artifacts/local/session.trace
cargo run --locked -p charon-sandbox -- --replay artifacts/local/session.trace --workers 1
```

記録は物質更新と同じスレッドでtickごとに保存する。再生はウィンドウ/GPUを起動せず、地形・位置・速度・判定/入力状態のchecksumを各tickで検証する。不一致は最初のtickを報告して失敗する。Rによる再開も記録される。保存先が既に存在すれば上書きせずエラーになる。記録のIO失敗でも動作を止める。

入力記録はデバッグ用で、IOとchecksum計算の負荷を含む。性能確認では有効にしない。形式は開発中のv1で、物理ルール変更後の同一結果を保証するゲームセーブ形式ではない。[動画と同時に記録した入力列](media/platformer-linux.trace)も同じコマンドで再生できる。Windowsの記録は `scripts/run.ps1 -Play -RecordInputs`。

## デバッグ操作

SandboxとPlatformerで共通。Pで停止/再開、Nで停止したまま1 tick進む、Iでセル調査を切り替える。調査中はマウス位置のセルを囲み、座標・Air/Sand/Wall・次tickの活動候補（AWAKE/SLEEP）を表示する。AWAKEは物質が今動いているという意味ではない。表示はサンプル用の英字HUD。

停止中も描画・入力・調査は続く。砂とキャラクターの時間は進まないが、マウスによる明示的な地形編集は直ちに反映する。停止中のジャンプ入力は次の更新で1回だけ使い、押し続ける掘削はコマ送り/再開時だけ実行する。Rの初期化は停止状態を保つ。

CSVの `runtime_tick` は入力受付/状態公開の回数、`simulation_tick` は実際の物質更新回数。停止中は前者だけ増える。`sim_hz` も後者から計算する。`paused`・`inspect_x/y`・`material`（0=Air、1=Sand、2=Wall、空欄=範囲外）・`next_tile_awake` を記録する。調査なしの場合も座標は空欄になる。`--play`・停止・セル調査中は約0.1秒ごとの記録。

[Linux操作記録（GIF）](media/debugger-linux.gif) · [再生用入力列](media/debugger-linux.trace)

`python3 scripts/capture-linux.py --sample debugger` で停止・編集・3回のコマ送り・リセット・再開と入力再生を確認し、録画を更新できる。Pause/Step/Inspectも `--input-log` の対象。既存の停止操作を含まないv1入力列も再生できる。

## Dialogue

```console
cargo run --locked --release -p charon-dialogue-sample
```

Spaceまたはクリックで進む。選択画面で1/2、Rで最初から、Escで終了。砂を依存に含めず、同じ実行基盤とwgpu描画基盤を使う小さな会話サンプル。文章表示はサンプル用の英字ビットマップで、日本語組版・音声・シナリオ編集機能はまだ含まない。

[Linux操作記録（GIF）](media/dialogue-linux.gif)

実入力で2つの分岐と再開を確認した約12秒の記録。収録条件はSandboxと同じ。`python3 scripts/capture-linux.py --sample dialogue` で再実行できる。両サンプルの `.csv` と `.json` を `docs/media/` に同梱する。JSONのbase_revisionは編集前のcommit、source_sha256は収録したengine/plugins/samples内のソース内容を示す。

## 検証

```console
cargo test --locked -p charon-runtime -p charon-sand -p charon-dialogue
cargo clippy --locked --workspace --all-targets -- -D warnings
python3 scripts/measure.py
```

measureは少量投入・連続投入・支えを壊す崩落・静止した堆積を、480×270/960×540セル、1/2/4ワーカーで比較する。各条件は60tickのウォームアップと240tickの計測。崩落は計測開始時に支えを削除する。CPU全体のp50/p95/p99/最大、更新/画像生成それぞれのp95、画像全体のバイト数（rgba_bytes_per_tick、実転送量とは別）、実使用ワーカー数、最終状態checksumをCSVへ出す。異なるワーカー数で最終状態が違えば失敗する。

`scripts/measure.py` はPython標準ライブラリだけで実行でき、CSVとコンパイラ/CPU/ソースhashを `artifacts/local/benchmarks/` へ保存する。Rust単体では `cargo run --locked --release -p charon-sand --example measure`。実時間60 HzやGPUの試験とは別。[初期実装のLinux計測CSV](benchmarks/sand-linux.csv)・[計測条件](benchmarks/sand-linux.json)は34db5d9時点の記録。差分化後の性能比較はまだ実施していない。複数ワーカーが常に速いという保証はない。

ウィンドウなしでshader出力を検証:

```console
cargo run --locked -p charon-sandbox -- --smoke
```

フルHD出力の全画素を完成したセル画像と照合する。画像ファイルは出力しない。途中のtickを受け取らないケース・静止・端の区画・リセットでも部分転送の出力を照合する。`--software` はソフトウェアアダプタを要求する。クラウドは `bash scripts/dev.sh smoke` で利用可能なSwiftShaderを選べる。これは実機のFPS検証ではない。

Linuxで操作試験とGIFの記録を再実行する場合は、`source scripts/env.sh` 後に `python3 scripts/capture-linux.py`。Xorgのdummyドライバ、X11/XTest、xauth、ImageMagick、Python/Pillowが必要。録画用ツールはエンジン本体の依存には含まれない。

既定の `cargo test` はGPU依存を持たないcore/pluginだけを対象にする。統合時は `cargo test --locked --workspace`。GitHub ActionsはLinux/Windowsでビルド・テスト・shader検証を行い、ActionsのArtifactsへ実行ファイルと検証記録を出力する。CIの実行結果は各runで確認する。

## 制約

サンプルはAir/Wall/Sandのみ。液体・剛体・保存・ゲーム用エディタは含まない。移動計算は32×32セルの活動領域へ限定する。画像も32×32の区画に分け、変化した区画だけ変換する。完了済み区画の不変データを共有するため、古いスナップショットの画素も変わらない。描画側は最後にGPUへ送った区画と照合するので、途中のtickを受け取れなくても変更が残る。

静止時は画素の再生成/転送がゼロになるが、区画ハンドルの受け渡し・照合と描画自体は続く。初回は全区画を送る。動いたセルを含む区画は丸ごと更新し、変更が多ければ転送量も増える。世界のストリーミングやGPU computeによる物質計算は未実装。

Windowsの実機操作と144 FPS/60 Hzの同時維持は、対象PCで別途確認する。CI成果物は開発確認用で、署名付きの製品配布物ではない。
