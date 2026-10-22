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

[Linux実行動画（MP4）](media/sandbox-linux.mp4) · [GIF](media/sandbox-linux.gif) · [スクリーンショット](media/sandbox-linux.png)

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

PowerShell起動では `artifacts/local/` に時刻付きCSVとRustバージョンを保存する。CSVは1秒区間の値で、tick処理時間は直近tickの値。更新/画像生成の時間は直近tick、upload_enqueue_ms・upload_bytes・acquire_ms・encode_present_msは直近描画のCPU側呼び出し時間と転送要求量。GPU実行時間や物理的な画面提示はまだ測定していない。VSyncやOSのスケジューリングも描画FPSへ影響する。

## Dialogue

```console
cargo run --locked --release -p charon-dialogue-sample
```

Spaceまたはクリックで進む。選択画面で1/2、Rで最初から、Escで終了。砂を依存に含めず、同じ実行基盤とwgpu描画基盤を使う小さな会話サンプル。文章表示はサンプル用の英字ビットマップで、日本語組版・音声・シナリオ編集機能はまだ含まない。

[Linux実行動画（MP4）](media/dialogue-linux.mp4) · [GIF](media/dialogue-linux.gif) · [スクリーンショット](media/dialogue-linux.png)

実入力で2つの分岐と再開を確認した約12秒の記録。収録条件はSandboxと同じ。`python3 scripts/capture-linux.py --sample dialogue` で再実行できる。両サンプルの `.csv` と `.json` を `docs/media/` に同梱する。JSONのbase_revisionは編集前のcommit、source_sha256は収録したengine/plugins/samples内のソース内容を示す。

## 検証

```console
cargo test --locked -p charon-runtime -p charon-sand -p charon-dialogue
cargo clippy --locked --workspace --all-targets -- -D warnings
python3 scripts/measure.py
```

measureは少量投入・連続投入・支えを壊す崩落・静止した堆積を、480×270/960×540セル、1/2/4ワーカーで比較する。各条件は60tickのウォームアップと240tickの計測。崩落は計測開始時に支えを削除する。CPU全体のp50/p95/p99/最大、更新/画像生成それぞれのp95、画像バイト数、実使用ワーカー数、最終状態checksumをCSVへ出す。異なるワーカー数で最終状態が違えば失敗する。

`scripts/measure.py` はPython標準ライブラリだけで実行でき、CSVとコンパイラ/CPU/ソースhashを `artifacts/local/benchmarks/` へ保存する。Rust単体では `cargo run --locked --release -p charon-sand --example measure`。実時間60 HzやGPUの試験とは別。[Linux計測CSV](benchmarks/sand-linux.csv)・[計測条件](benchmarks/sand-linux.json)も参照。複数ワーカーが常に速いという保証はない。

ウィンドウなしでshader出力を検証:

```console
cargo run --locked -p charon-sandbox -- --smoke --output artifacts/local/smoke.png
```

フルHD出力の全画素を、完了したシミュレーションのセル画像と比較し、PNGを保存する。`--software` はソフトウェアアダプタを要求する。クラウドは `bash scripts/dev.sh smoke` で利用可能なSwiftShaderを選べる。これは実機のFPS検証ではない。

Linuxで操作試験とMP4/GIF/PNGの記録を再実行する場合は、`source scripts/env.sh` 後に `python3 scripts/capture-linux.py`。Xorgのdummyドライバ、X11/XTest、xauth、ImageMagick、FFmpeg、Python/Pillowが必要。録画用ツールはエンジン本体の依存には含まれない。

既定の `cargo test` はGPU依存を持たないcore/pluginだけを対象にする。統合時は `cargo test --locked --workspace`。GitHub ActionsはLinux/Windowsでビルド・テスト・shader検証を行い、ActionsのArtifactsへ実行ファイルと検証画像を出力する。CIの実行結果は各runで確認する。

## 制約

サンプルはAir/Wall/Sandのみ。液体・剛体・保存・ゲーム用エディタは含まない。移動計算は32×32セルの活動領域へ限定するが、描画用スナップショットは全セルを変換・転送する初期実装。大規模世界向けの差分転送は今後の対象。

Windowsの実機操作と144 FPS/60 Hzの同時維持は、対象PCで別途確認する。CI成果物は開発確認用で、署名付きの製品配布物ではない。
