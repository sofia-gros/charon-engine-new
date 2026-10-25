# M1 ゲームサンプル：Gem Run

Windows 11でM1のCore / Plugin基盤を確認するための小さな2Dゲームです。
60秒以内に黄色い宝石を6個集めてください。ピンクの移動する障害物に当たるとHPが減り、HPが0になるか時間切れで失敗します。

## Windows 11で実行する

### 実行ファイルを使う

1. リポジトリの [Actions](https://github.com/sofia-gros/charon-engine-new/actions/workflows/ci.yml) で、対象コミットの成功したCIを開く。
2. Artifacts の `charon-M1-win11-x64` をダウンロードし、ZIPをすべて展開する。
3. `run.cmd` または `charon-m1-sample.exe` を実行する。

Windows x64向けです。実行用の画像・フォント・Rustのインストールは不要です。CIでビルド・自動テストを実行しますが、Windows 11実機の画面・入力確認は下のチェックリストで行ってください。

### ソースから起動する

Rust stable（MSVC）と、Visual Studio Build Toolsの「C++によるデスクトップ開発」を用意し、リポジトリのルートで実行します。

```powershell
cargo run --release -p charon-m1-sample
```

## 操作

| 操作 | キー |
| --- | --- |
| 移動 | WASD / 矢印キー |
| 一時停止・再開 | P |
| 最初からやり直す | R |
| 終了 | Esc / ウィンドウを閉じる |

下部に実際のPlugin順序とフレーム番号、上部に得点・HP・残り時間・固定tick数を表示します。ゲーム開始時はフォーカスを合わせて操作してください。

任意のPulse Pluginを登録しない構成も実行できます。宝石の周囲の点滅がなくなり、HUDに `PULSE OFF` と表示されます。

```powershell
.\charon-m1-sample.exe --no-effects
```

## Windows 11実機の確認項目

- [ ] ゲーム画面が開き、文字・タイル・プレイヤー・宝石が表示される
- [ ] WASD / 矢印で移動でき、壁を通り抜けない
- [ ] 宝石を取ると消え、得点が1だけ増える
- [ ] 移動する障害物に当たるとHPが減り、短時間の連続ダメージは防がれる
- [ ] 6個集めると勝利、時間切れまたはHP0で失敗になる
- [ ] Pで停止し、位置・残り時間・ゲームtickが止まる。再度Pで再開する
- [ ] Rで位置・宝石・得点・HP・残り時間が初期状態に戻る
- [ ] ウィンドウをリサイズしても表示できる
- [ ] `--no-effects` でもゲームが成立し、点滅とPulseの登録だけがなくなる
- [ ] Esc / 閉じるボタンで終了し、コンソールの終了ログが初期化の逆順になる

確認結果を共有するときは、コミット、Windowsのバージョン、画面倍率、操作、期待した結果と実際の結果を記録してください。

## M1で確認する設計

| Plugin | 役割 |
| --- | --- |
| Game | ゲーム状態・移動・収集イベント・勝敗。固定tickで動く |
| Score | Gameに依存し、収集イベントから得点を更新する |
| Pulse（任意） | フレームごとに表示用の演出値を更新する |
| View | Game / Scoreの状態と任意の演出値から表示用バッファを作る |

登録は意図的にViewから始めますが、Coreが依存関係を解決してGameから初期化します。終了は逆順です。ウィンドウの作成と入力取得はサンプルのホストが担当します。

このゲームはFalling Sand、GPU計算、正式なTilemap / Rendering / Input Pluginsに依存しません。表示はサンプル専用のソフトウェア描画で、将来の機能の先行実装ではありません。

## 自動確認

```sh
cargo run -p charon-m1-sample -- --smoke-test
cargo run -p charon-m1-sample --no-default-features -- --smoke-test
cargo test -p charon-m1-sample
```

スモークテストはウィンドウなしで、任意Pluginあり・なし、依存順序による初期化、宝石収集・得点・勝利、再開始、描画バッファ生成、終了を確認します。プレイヤー位置をスクリプトで設定するため、実機での入力・表示確認の代わりにはなりません。

開発用に、ウィンドウなしで最初のフレームをPPM画像として保存できます。

```sh
cargo run -p charon-m1-sample --no-default-features -- --snapshot m1.ppm
```
