# 導入と使い方

## 必要な環境

- Rust stable、Git。
- Windows 11: MSVC版RustとVisual Studio Build Toolsの「C++によるデスクトップ開発」。
- Linux: Cリンカ、X11、VulkanまたはOpenGL ESドライバ。ウィンドウ不要のCPU検証も可能。
- `rust-toolchain.toml` はstableを指定する。更新は `rustup update stable`。依存バージョンはCargo.lockで管理する。

```console
git clone https://github.com/sofia-gros/charon-engine-new.git
cd charon-engine-new
```

開発ブランチを確認する場合は、そのブランチをcheckoutしてから起動する。

## Sandbox

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

PowerShell起動では `artifacts/local/` に時刻付きCSVとRustバージョンを保存する。CSVは1秒区間の値で、tick処理時間は直近tickの値。全tickの分布やGPU実行時間、物理的な画面提示はまだ測定していない。VSyncやOSのスケジューリングも描画FPSへ影響する。

## 検証

```console
cargo test --locked -p charon-runtime -p charon-sand
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo run --locked --release -p charon-sand --example measure -- 4
```

measureは480×270セルへ規則的に砂を追加し、120tickのウォームアップ後600tickのCPU処理時間を測定する。最後の砂数・活動量も出力する。同じ条件でワーカー数だけ変えて比較する。実時間の60 Hz試験とは別。

ウィンドウなしでshader出力を検証:

```console
cargo run --locked -p charon-sandbox -- --smoke --output artifacts/local/smoke.png
```

フルHD出力の全画素を、完了したシミュレーションのセル画像と比較し、PNGを保存する。`--software` はソフトウェアアダプタを要求する。クラウドは `bash scripts/dev.sh smoke` で利用可能なSwiftShaderを選べる。これは実機のFPS検証ではない。

既定の `cargo test` はGPU依存を持たないcore/pluginだけを対象にする。統合時は `cargo test --locked --workspace`。GitHub ActionsはLinux/Windowsでビルド・テスト・shader検証を行い、ActionsのArtifactsへ実行ファイルと検証画像を出力する。CIの実行結果は各runで確認する。

## 制約

サンプルはAir/Wall/Sandのみ。液体・剛体・保存・ゲーム用エディタは含まない。移動計算は32×32セルの活動領域へ限定するが、描画用スナップショットは全セルを変換・転送する初期実装。大規模世界向けの差分転送は今後の対象。

Windowsの実機操作と144 FPS/60 Hzの同時維持は、対象PCで別途確認する。CI成果物は開発確認用で、署名付きの製品配布物ではない。
