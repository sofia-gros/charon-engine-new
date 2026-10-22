# 参考エンジンのコード調査

過去の調査資料。ここでの設計提案は現行方針ではない。現在の合意は [engineering-plan.md](../engineering-plan.md) を参照。

調査日: 2026-10-22。対象は2D・シングルプレイ、フルHD描画144 FPS、シミュレーション60 Hz、CPUマルチスレッドとGPUシェーダー対応。

## 調査の範囲と再現性

3リポジトリを実際にcloneし、以下のコミットのソースを読み取った。Git履歴を含まないソースの完全なスナップショットも `/workspace/library-files/charon-references/` に配置した。バージョンと内容のSHA-256は [references.lock.json](../references.lock.json) に記録した。

| リポジトリ | 調査コミット | 位置づけ |
| --- | --- | --- |
| PieKing1215/FallingSandSurvival | `9b64a9f69e3f6293691f6496c6c4934e9ca1f724` | C++のゲーム・エンジン |
| PieKing1215/FallingSandEngine | `8ea0463b858105277000bd562e4cf638cfdca32f` | そのRust書き直し版 |
| Hugo-Dz/rust-sandbox | `78ba5fb4f01727c171b1a2d524d1a0e8458fd902` | Bevyの小規模学習デモ |

`python3 scripts/references.py` で固定版を復元・検証できる。既存ファイルが異なる場合は上書きしない。参考コードは新エンジンの依存関係に加えない。

実施したのは静的な構造・更新経路の調査と、rust-sandboxの移動関数を切り出した小さな実行実験。3作品の完全ビルド、GPU実行、FPS比較は行っていない。以前の自作エンジンは未提供なので、そこで1 FPS以下になった直接原因は未特定。以下の「懸念」は、その作品の不具合や開発停止原因を実測で証明したという意味ではない。

## 開発停止について作者が説明していること

### C++版: メモリ管理とビルド環境の保守負担

[README](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/README.md#L13) は、開発停止と別言語への書き直しを述べている。

さらに [Issue #4の2022-02-21の作者コメント](https://github.com/PieKing1215/FallingSandSurvival/issues/4#issuecomment-1047182838) では、Rustを選ぶ理由として、C++を書き始めたころの経験不足から「95%」が生ポインタを使っていること、ビルド環境が時間とともに壊れ、直す手間が大きいこと、ランダムなクラッシュやメモリリークの調査が難しかったことを挙げている。「95%」は作者の表現で、今回の計数結果ではない。

これはC++一般の欠点という断定ではなく、当該実装とツールの運用に関する本人の説明。Rust化だけでスケジューラや所有権の設計問題も解決する、という話ではない。

### Rust版: 遊びに必要な機能とエンジンの境界が合わなくなった

[README冒頭](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/README.md#L3) は、ゲームプレイを設計した結果、追加したい機能に大規模な書き直しが必要になり、その機能を最初から組み込んだ別プロジェクトへ移ったと説明している。

具体的に何の機能だったかは、この説明にはない。並列処理の不健全性や性能不足が開発停止の直接原因だった、と結びつける証拠も確認していない。「作者がゲーム開発そのものを断念した」も誤読になる。

新エンジンへの教訓は、先に小さなゲームとして入力・破壊・足場・保存を試し、セル、剛体、プレイヤー、描画の責任境界を固定すること。未確定の全機能を先回りして実装することではない。

## コードから確認できる設計と注意点

### FallingSandSurvival

| 観察した箇所 | 確認できる内容 | Charonに採用する対策 |
| --- | --- | --- |
| [world.cpp:1091](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/FallingSandSurvival/world.cpp#L1091) | `tick` に6反復×4フェーズ。対象領域のチャンクを走査し、visited配列を繰り返し初期化。前tickのdirtyだけを調べる案がTODOになっている | 活動セル数・走査セル数・初期化量・フェーズ待ち時間を計測し、変化のない領域を休止する。描画dirtyとシミュレーションの活動範囲を分ける |
| [world.cpp:63](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/FallingSandSurvival/world.cpp#L63) | 用途別に固定サイズの複数スレッドプール | 利用可能CPU数と実測からワーカー数を決める。ワーカーを増やすだけで速くなると仮定しない |
| [Game.cpp:2440](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/FallingSandSurvival/Game.cpp#L2440) | 粒子更新がtiles/dirtyを書き換えるので完全にはスレッドセーフでない、というコメント。続く処理で粒子とオブジェクト境界のジョブを起動 | 各フェーズの読み取り対象と書き込み所有者を明記。独立したワーカーが共有の地形を直接変更しない |
| [Game.cpp](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/FallingSandSurvival/Game.cpp) / [world.cpp](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/FallingSandSurvival/world.cpp) | それぞれ4429行/3711行で、更新・描画・入力・地形等が広く集約されている | 行数だけを品質評価にしない。変更が複数のシステムに及ぶ経路を契約テストで保護し、責任単位で分ける |
| [setup.sh](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/setup.sh) / [Conan設定](https://github.com/PieKing1215/FallingSandSurvival/blob/9b64a9f69e3f6293691f6496c6c4934e9ca1f724/FallingSandSurvival/conanfile_linux.txt) | 古い依存版、追加レジストリ、対話的セットアップ、レシピの書き換え、外部SDKの手作業設定 | 核はCargoだけでビルド可能にする。描画以外のSDKを必須にせず、ツールチェーンとlockfileを固定する |

既にチャンクやプロファイラを使っている。「チャンクを導入すれば解決」という単純な分析にはならない。境界処理、走査回数、粒子への変換、描画転送まで含めて検証する必要がある。

### FallingSandEngine

採用したい点は、チャンク、dirty矩形、用途別クレート、プロファイラ用の区間、剛体のローカルなピクセル領域。次の前提はそのまま引き継がない。

| 観察した箇所 | 確認できる内容 | 設計上の意味と対策 |
| --- | --- | --- |
| [particle.rs:152](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_common/src/game/common/world/particle.rs#L152) | `ForceSendSync` と生ポインタを使う。1 tickの移動距離がチャンク半分以下という安全性の前提が未強制、と明記 | データ競合の可能性に関する強い根拠だが、この調査でUB発生を実行確認したわけではない。共有可変参照を避け、移動距離・書き込み範囲をAPIで制約する |
| [chunk_handler.rs:813](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_common/src/game/common/world/chunk_handler.rs#L813) | 4フェーズと `UnsafeCell` を使用。更新の到達範囲が未チェックで、aliasingへの懸念を作者も記載 | フェーズ数そのものは安全性の証明ではない。まず読み取りスナップショット→提案→競合解決→一括適用で所有権を明確にする |
| [world.rs:400](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_common/src/game/common/world/world.rs#L400) とREADMEのtick疑似コード | 剛体/エンティティの位置に仮のObjectセルを書き、砂・粒子を更新してから消し、剛体内のセルと衝突形状を更新する | 世界セルと一時的な占有情報が混ざる。新設計は占有マスクを別に持ち、物質の移動は明示した移管イベントにする |
| [material/mod.rs:32](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_common/src/game/common/world/material/mod.rs#L32) / [registry.rs:15](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_common/src/game/common/registry.rs#L15) | 各セルに文字列由来IDの `Arc<String>`、色、光等を保持し、移動時にcloneする | 帯域や参照カウントのコスト候補。今回その寄与は未計測。小整数のMaterialIdと共有テーブル、必要に応じた付加配列を使う |
| [fs_common/Cargo.toml:19](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_common/Cargo.toml#L19) | 共通クレートにglium、ECS、物理、画像、メッシュ生成等の依存が入る | 共通ロジックのテストでも広い依存集合が必要になる。GPUのないsim-coreと、GPU専用のclientを切り離す |
| [chunk.rs:16](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_common/src/game/common/world/chunk.rs#L16) | 100セルのチャンク幅とライティング計算の制約が関連づけられている | セル保管単位とGPUの作業単位を別の設定にする |
| [client chunk.rs:227](https://github.com/PieKing1215/FallingSandEngine/blob/8ea0463b858105277000bd562e4cf638cfdca32f/fs_client/src/world/chunk.rs#L227) | `mark_dirty` で背景・光もdirtyになる | 材質・背景・光・衝突の変更理由を分ける。GPU転送と再メッシュの増幅を計測する |

dirty矩形や並列化は実装済みなので「最適化が何もない作品」とするのは誤り。`dynamic()` の判定をclone前に置く効果について作者のコメントもあるが、そこに書かれた割合を今回の性能測定として引用しない。

### rust-sandbox

この作品はREADMEでも「物理を使わず、Rustを学ぶため」と説明している。用途に照らせばシンプルさに価値がある。

- [constants.rs](https://github.com/Hugo-Dz/rust-sandbox/blob/78ba5fb4f01727c171b1a2d524d1a0e8458fd902/rust/src/utils/constants.rs): セル解像度80×60、ウィンドウ640×480。フルHD全画素シミュレーションの設計事例ではない。
- [grain.rs:41](https://github.com/Hugo-Dz/rust-sandbox/blob/78ba5fb4f01727c171b1a2d524d1a0e8458fd902/rust/src/grain.rs#L41): 1粒につきSprite/Transform等を持つエンティティを生成する。大きなセル数で同じ構造を使うことは避ける。Bevyを使う場合も地形全体をResourceにする選択肢がある。
- [main.rs:53](https://github.com/Hugo-Dz/rust-sandbox/blob/78ba5fb4f01727c171b1a2d524d1a0e8458fd902/rust/src/main.rs#L53) / [grain.rs:148](https://github.com/Hugo-Dz/rust-sandbox/blob/78ba5fb4f01727c171b1a2d524d1a0e8458fd902/rust/src/grain.rs#L148): `Update` ごとのカウンタで移動頻度を制御。実時間60 Hzから独立した固定tickではない。またシステムの実行順の依存は明示されていない。
- [blood.rs:16](https://github.com/Hugo-Dz/rust-sandbox/blob/78ba5fb4f01727c171b1a2d524d1a0e8458fd902/rust/src/blood.rs#L16): 全粒子が同じ不変Gridを参照し、GridPosition/Transformだけを動かす。別粒子による同じ移動先の予約を行わない。
- [grid.rs:52](https://github.com/Hugo-Dz/rust-sandbox/blob/78ba5fb4f01727c171b1a2d524d1a0e8458fd902/rust/src/grid.rs#L52): 別システムで前座標を消して現座標を書く。セル占有とエンティティ位置という二重の状態を整合させる責務が生じる。

移動先の競合は実行実験でも確認した。`source scripts/env.sh` の後に `python3 scripts/probe-reference.py` を実行すると、固定版のGrid/移動関数を抽出してrustcでコンパイルする。BevyのTransform等だけを小さな型で代用し、両側の床に支えられたBlood `(2,1)` と `(4,1)` を空の `(3,1)` に向かわせると、両方が同じ座標を選ぶ。これは関数の競合解決がないことを示す限定的な実験であり、完全なBevyゲームの動作試験ではない。

「読み取り/書き込みを分ける」だけでは不十分で、複数の移動提案の競合解決も必要、という反例になる。

## Codexでの開発手順の参考例

[openai/codex](https://github.com/openai/codex/tree/2f3fb111be71d277c80bd5b96d9019748b921f5a) も取得して、[justfile](https://github.com/openai/codex/blob/2f3fb111be71d277c80bd5b96d9019748b921f5a/justfile) の用途別コマンドと引数で対象を絞るテスト、[rust-ci.yml](https://github.com/openai/codex/blob/2f3fb111be71d277c80bd5b96d9019748b921f5a/.github/workflows/rust-ci.yml) の変更領域判定と速いPR向けジョブを確認した。これはCodex利用時の速度改善を実験したベンチマークではなく、公開された運用例。

採用するのは、対象を明示したコマンド、速い日常チェックと統合チェックの分離、実行結果の明示。巨大なビルド基盤や全ての補助ツールまで導入する必要はない。Charonでは今の規模に合わせてBash/Cargoで足りる構成にした。

## ライセンスと残る不明点

FallingSandのREADMEはコードをBSD-3-Clause、アートをCC BY-NC 4.0としている。Rust版のCargoメタデータには `ARR` の表記もあり、コードを実際に移植する場合は対象ファイルの条件を再確認する。rust-sandboxはMIT。今回は調査用コピーのみで、参考作品のアートを製品に組み込んでいない。

不明なのは、Rust版で追加したかった具体的なゲーム機能、作者の非公開な判断経緯、各作品の同じハードウェアでの性能、以前の自作エンジンの低FPSの直接原因。これらを推測で埋めず、今回の設計を小さなゲーム試作・性能測定・境界テストで評価する。
