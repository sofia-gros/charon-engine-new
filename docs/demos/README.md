# M実装事例の操作GIF

各マイルストーンの完了時に、操作できるサンプルを録画し、GIFと録画情報をGit管理します。READMEの「M実装事例」から表示します。進行中の事例は、完了済みと区別して掲載します。

| 事例 | 内容 | 状態 |
| --- | --- | --- |
| [M1](./m1-demo.gif) / [録画情報](./m1-demo.json) | 移動、宝石取得、一時停止、再開、リセット | M1実装事例 |
| [M2](./m2-demo.gif) / [録画情報](./m2-demo.json) | 砂・水・雲・窒素・土、障壁解除、砂への変換、安息角変更 | M2実装完了 |
| [M3](./m3-demo.gif) / [録画情報](./m3-demo.json) | 宝石取得、タイルの設置・破壊、砂・水への変換、Shader設定 | M3実装完了 |
| [M3 Tilemap](./m3-tilemap-demo.gif) / [録画情報](./m3-tilemap-demo.json) | 同じ操作をFalling Sandなしで実行 | M3単体利用例 |

現在のGIFはLinux / Xvfb上で実際のネイティブウィンドウを表示し、XTestでキー・マウス操作を送り、ffmpegの画面キャプチャで録画したものです。プログラムで生成したフレームの合成ではありません。Windows 11実機の操作確認とは区別してください。

## 完了時の手順

1. 対象サンプルのテストとビルドを確認する。
2. そのMの主要機能と操作結果が分かる短い操作列を決める。
3. 実際のウィンドウを録画し、GIFへ変換する。
4. GIFを再生し、操作結果、文字の可読性、ループ、表示範囲を確認する。
5. `mN-demo.gif` と同名の `mN-demo.json` を保存する。
6. READMEの「M実装事例」に埋め込み、ROADMAPと事例の状態を更新する。
7. GIF・JSON・手順を同じコミットでpushする。

録画情報には、対象ソースのコミット、録画日時、OS・表示環境、起動コマンド、操作内容、Windows 11での確認の有無を含めます。GIFだけで、動作検証や性能目標をすべて達成したとは扱いません。

GIFは768px程度の幅・12fps程度を目安にし、短い操作列に絞ります。5MiB以内を目指し、画面全体を過度に縮小して文字が読めなくなる圧縮は避けます。差し替え前の版はGit履歴から取得できます。

## Linuxでの再録画

Xvfb、ffmpeg、libX11、libXtstとPython 3を用意します。録画スクリプト自体には追加のPythonパッケージは不要です。

```sh
cargo build --release -p charon-m1-sample -p charon-m2-sample -p charon-m3-sample
python scripts/record_milestone.py m1
python scripts/record_milestone.py m2
python scripts/record_milestone.py m3
python scripts/record_milestone.py m3-tilemap
python scripts/check_demo_assets.py
```

Xvfbの場所やディスプレイ番号を指定する場合：

```sh
python scripts/record_milestone.py m1 --xvfb /path/to/Xvfb --display :97
```

使用中のディスプレイは選ばないでください。スクリプトは専用Xvfbとサンプルを起動し、キー・マウス操作、キャプチャ、GIF変換を行って終了します。サンプルの操作やウィンドウ名を変更した場合は、録画手順も更新します。

Windowsで録画した場合も同じGIF・JSON構成で保存し、録画環境を正しく記録してください。Windows 11でのサンプル起動コマンドは各サンプルのREADMEにあります。
