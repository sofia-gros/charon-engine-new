# charon_engine

charon-engineの任意の高レベル構成APIです。既定依存は `charon_core` だけです。Featureを有効にしてもPluginを自動登録しません。ウィンドウ・world・物理・GPUを暗黙に生成しません。

```toml
[dependencies]
charon_engine = { git = "https://github.com/sofia-gros/charon-engine-new", features = ["interaction", "scenes"] }
```

実用時には検証したコミットを `rev` で固定してください。現在は開発中の0.1 APIで、crates.ioへの公開・安定性保証はまだありません。

| Feature | 公開する部品 |
| --- | --- |
| なし | AppBuilder、Startup、Rates、Worker起動とCore直接API |
| input / camera / scenes | 各ライブラリとPlugin。scenesはSceneCatalogも提供 |
| assets | CPUアセット、Assetsセット |
| interaction | input + camera + assets、Interaction2Dセット |
| lighting | 光学データ、LightingConfig、品質プリセット、照明結果の取得、CPU参照Plugin |
| tilemap | Tilemap直接APIとPlugin |
| render | GPUに依存しない描画APIとPlugin |
| gpu | render + wgpu/winitバックエンド |
| physics | 物理直接API |
| sand | physics + Falling Sand Plugin |
| bridge | sand + tilemap + 双方向連携API |

高レベルAPIは既存のApp / SceneManagerを返します。World・Plugin・Backendを独自に変更でき、独自Pluginも `.plugin()` / `.boxed_plugin()` で登録できます。各部品だけを使う場合は個別crateへ直接依存してください。

[全体のAPI・移行ガイド](../../docs/api-guide.md) / [比較ゲーム](../../samples/m6-api-lab/README.md)
