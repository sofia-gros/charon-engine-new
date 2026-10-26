# HDR照明の検証記録

2026-10-26。実行環境はLinux / SwiftShader Device (Subzero)。以下はソフトウェアVulkan上のPath検証であり、RTX実機の画質・性能の測定結果ではありません。

- `cargo test --workspace --all-features`：145テスト成功、失敗・警告なし。
- `cargo check -p charon-rt-sample --no-default-features`：成功。
- `charon-tracing-check`：空、壁、開口、水、閉じた壁の5ケースでCPU/GPU比較成功。互換出力の最大差は約2.18e-5。未clampのHDR成分についても誤差2e-3以下を確認。
- HDR発光（1を超える値）、材質発光だけによる照明、光源除去直後の残光消去、Liteの既存制約とモード切り替えも成功。
- CPUテストで粒子の隙間、world距離による吸収、開始セル・prismの出口、内部水面の除去、疎な不透明粒子、影響範囲の履歴保持、静止水の蓄積、サンプル上限、ゼロ距離予算を確認。
- RTX用WGSL、薄い形状の正面・背面・共有面、厚みの違う隣接セルの境界は静的に検証。実機Ray Query・動的BVH更新は未検証。

上記は初回実装時の記録です。以下の追記にノイズ修正後の結果を記載します。

## オフスクリーン画像と動画

```sh
cargo run -p charon-tracing-check -- --gallery target/lighting-gallery --1080p
ffmpeg -framerate 12 -i target/lighting-gallery/motion-%03d.png -c:v libx264 -pix_fmt yuv420p target/lighting-gallery/motion.mp4
```

96×64のworld入力を1920×1080に描画。静止画は8 samples/update、3拡散反射、64蓄積samples、Pathの1反射・4透過予算。固定カメラ・露出1で、実際のHDR合成・ブルーム・トーンマップを実行しました。動的24フレームでは炎と不透明粒子を動かしています。

出力は `stage-0.png`（拡散のみ）、`stage-1.png`（自己発光）、`stage-2.png`（反射・透過）、`motion.mp4` です。旧バージョンとの比較ではなく、同じ現在のシーンで効果を段階的に有効化した比較です。これらは `target` 内の再生成可能な成果物です。

最終動的フレームの測定例：CPU submit 約4.5ms、GPU trace+resolve 約1303ms、trace buffer/texture概算1.55MiB、反射・透過成分の最大値2.82。GPU時間はCPUで動くソフトウェアVulkanの値です。表示・後処理・画像readbackを除きます。デモの320×192入力、RTXの16 samples/update、ハードウェアBVHの構築費用・メモリ量を代表しません。

## RTX実機で確認する項目

```shc
cargo run --release -p charon-tracing-check -- --gallery target/lighting-gallery-rtx --1080p --rtx
cargo run --release -p charon-rt-sample -- --config samples/rt-light-lab/config/rtx.json --gpu-check
```

実deviceのprobeと候補dispatchが成功していること、火・水・灰の動的更新、カメラ移動、壁の編集、方式の切り替えを確認します。1080pでのtrace時間、CPUの形状準備時間、面数、buffer概算を記録し、必要ならGPU profilerで後処理とBVH構築も測定します。非対応・失敗をPathの代替結果でRTX成功と表示する処理はありません。

## ノイズ修正

- 光源の一様な抽選を光束CDFに変更。小さな強い光源を低確率で選んだ時の過大な寄与を抑制。
- 直接光は有限の固定quadrature、滑らかな反射・透過はFresnel重みによる決定的な分岐追跡に変更。反射先の拡散面を再度ランダムに追う処理を除去。
- 間接拡散光に独立した過大サンプル制限を追加。自己発光・直接光・反射のHDRレンジを維持。
- 鏡面成分をぼかして斑点を隠す処理を除去。残る間接拡散光には従来の材質境界を考慮した再構成を使用。
- 低サンプルの静止反射、光源の確率分布、間接光だけの上限を検証する回帰テストを追加。

`--gallery target/lighting-noise-fix --1080p --noise-check` で8 samples/updateの最初の2フレームを保存します。GPUで反射・透過のフレーム間最大差0を確認します。間接色移りの近似についてはtracing APIを参照してください。修正前の参考画像は `target/lighting-before-noise-fix` に保持しています。

修正後の確認：全workspace 148テスト成功・警告なし、Liteビルド成功、Path CPU/GPU比較成功（互換出力の最大差約3.26e-5）。8 samples/updateの反射・透過のフレーム間最大差はGPUで0でした。画像は `target/lighting-noise-fix/low-samples-0.png` と `low-samples-1.png` です。RTX実機の結果ではありません。

## 実デモでのdevice lost・低サンプルノイズ報告への対処

RTX 4060でPath約59.6 ms、動的場面の強いノイズ、RTX起動時の `Parent device is lost` が報告されました。従来の小さい合成シーンと静止反射の検証では、これらの実デモの品質・安定性を保証できていませんでした。**RTX 4060での起動・速度・クラッシュ解消は依然として未確認です。** `Device::poll` のdevice lostはGPU喪失の通知であり、原因がwatchdogかドライバーかをこのログだけでは断定できません。releaseの `panic=abort` により、panic時はプロセスが異常終了します。

今回の変更：

- `submit` の即時計算をやめ、計算HzとGPU完了を確認した `tick` で最新入力だけを投入。
- HDR dispatchを8行ずつのsubmissionに分割。形状解像度と総サンプル数は維持。
- 表面の全奥行きより外に出て戻らないレイの探索・透過計算を省略。
- 変化していない直接光・反射をキャッシュ。デモの材質構築も照明更新頻度に制限。
- 空の直接光を固定積分。Liteは光源の直接光も固定積分。
- 動的粒子の周囲では間接光の履歴を短く保持。変化したセルと静的壁編集の影響範囲は即時破棄。間接光には短い応答遅れが残る近似。
- Pathの固定光源サンプル上限を32から8へ削減。RTXは64を維持。

今回確認した範囲：全workspace 151テスト成功、RTXなしビルド成功、Path CPU/GPU数値比較（互換出力の最大差約3.26e-5）、WGSL検証、実デモのマップ・材質アダプターを使ったPath/LiteのGPU実行、動く砂・炎を与えたPath 4フレームの有限HDR出力、静止反射・透過のフレーム間最大差0。

実デモPathは320×192・65光源、動的入力では66光源です。SwiftShaderで静止初回約3.25秒、次の更新約1.30～1.36秒、動的更新約2.95～3.15秒を計測しました。これは**ソフトウェアGPUの検証値**であり、RTX 4060のFPS改善を示す数値ではありません。windowの表示・編集・モード切替と、RTX実機での長時間試験は未確認です。合成シーンの今回の比較画像は `target/lighting-runtime-fix/low-samples-{0,1}.png` です。

実機用の追加コマンド：

```sh
cargo run --release -p charon-rt-sample -- --trace-check --lighting rtx --motion --frames 8
```

このコマンドはadapterの対応表示だけで成功にせず、実デモサイズの材質・形状・反射計算とreadbackを実行します。起動した場合も、通常windowを使った実行確認は別に必要です。

## RTX実機での再失敗と段階診断

ユーザーのRTX 4060で、処理分割後も `--trace-check --lighting rtx --motion --frames 8` が最初のframe完了前にdevice lostで終了しました。前回の対策では解消していません。window・UIを使わなくても再現することは確認できましたが、BVH構築、ray query、輸送シェーダー、GPU timeoutのどれが原因かは未確定です。

診断を次の順に分離しました。各段階の前後をログに出し、GPU readbackで完了を確認してから進みます。

1. 実デモのHDR prism meshでBLAS/TLASだけを構築し、ray dispatchなしで完了を待つ。
2. HDRの大きなシェーダーから独立した小さなシェーダーで、既知の壁正面へ光線1本を発射。距離・セル番号・法線軸を検証する。
3. 実デモサイズで直接光、間接光、表面反射・透過をそれぞれ1 sampleで実行する。
4. 全効果1 sample、次に元の16 samples/updateのRTX品質で実行する。

これに合わせ、0 bounceでも最初の間接光を計算していた処理と、表面予算0でも最初の反射・透過レイを発射していた処理をCPU/GPU双方で修正しました。通常の非ゼロ品質プリセットは維持します。ゼロ予算の診断が効果を正しく無効化する回帰テストを追加しています。

今回の確認は、152テスト、独立probe WGSLを含む静的検証、Pathでの全照明段階のGPU実行、CPU/GPU整合性、RTXなしビルドです。実RTXでの独立probe・各段階の結果は未取得で、クラッシュ解消は未確認です。
