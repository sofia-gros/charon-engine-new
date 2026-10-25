# 物質の登録・安息角・独自更新

`charon_physics` の `MaterialRegistry` に定義を登録して物質を追加します。エンジンの物質enumやレンダラーを編集する必要はありません。砂・水・油などの10物質は、利用開始時に用意される標準プリセットです。

## 名前・密度・移動・色を登録する

```rust
use charon_physics::sim::{
    grid::Grid,
    material::CellData,
    rules::{MaterialDefinition, MaterialRegistry, Movement},
};

let mut materials = MaterialRegistry::default();
let nitrogen = materials.register(MaterialDefinition::new(
    "Nitrogen", Movement::Gas, 1.165, [136, 180, 220, 255],
))?;
let cloud = materials.register(MaterialDefinition::new(
    "Cloud", Movement::Gas, 0.7, [205, 222, 240, 255],
))?;
let soil = materials.register(
    MaterialDefinition::new("Soil", Movement::Granular, 1600.0, [125, 84, 48, 255])
        .with_repose(38.0)?,
)?;

let mut grid = Grid::new(320, 200);
grid.set_materials(materials);
grid.set(100, 150, CellData::new(nitrogen, 0));
grid.set(120, 150, CellData::new(cloud, 0));
grid.set(140, 20, CellData::new(soil, 17));
```

上記の `?` は `Result` を返す初期化処理の中で使います。Pluginへ接続する場合は、このGridを `Simulation::from_grid(grid, seed)` に渡してから `FallingSandPlugin` に登録できます。`Simulation::new(...).with_materials(materials)` でも定義を取り込めます。

`Movement` は `Empty`・`Solid`・`Granular`・`Liquid`・`Gas` です。密度はkg/m³を目安とする正の有限な小数で定義でき、気体は空気より軽ければ上へ、重ければ下へ移動します。これはセルオートマトンの移動モデルで、雲の生成過程、圧力、化学的性質を名前から自動生成するものではありません。

表示は物質IDに対応する登録色を参照します。`MaterialRegistry::color()` と公開snapshotの `palette` を使えるため、独自物質の追加でレンダラーに分岐を増やす必要はありません。

## 安息角と摩擦

粒状物質には `with_repose()` / `set_repose()` で、有限な `0°以上90°未満` の角度を指定できます。定義した物質IDに対して `set_friction()` で静止摩擦係数を指定することもできます。摩擦係数と安息角は `μ = tan(θ)` の関係として扱います。

標準は砂32°・砂利40°です。設定は物質ごと・ワールドごとに独立しています。密度や液体の横方向の広がりも `set_density()` / `set_spread()` で調整できます。更新後は `Grid::set_materials()` で休止領域を起こします。稼働中のSimulationでは `Operation::SetRepose` をコマンドとして送れます。

表面の高さを列ごとに参照し、16セル離れた表面との勾配が設定を超えた場合に上の粒子を隣へ移します。高摩擦の場合は対角移動も抑えます。セルを増減させず、2バイトの `CellData` を維持しています。表面のキャッシュは列単位の独立配列です。

有限の格子と参照距離による近似であり、任意の小さな砂山が指定角度に正確に一致するわけではありません。特に極端な角度や狭い地形では量子化・境界の影響があります。32°・40°・45°・60°を、平面・斜面・段差、2つのseedで測定した[結果](./performance/m2-repose.json)を保存しています。32°設定の標準検証は左右とも±4°以内、質量保存、静止を確認します。

## 独自の反応・寿命・動作を書く

標準の移動を置き換える場合は `with_update()` に更新関数を渡します。

```rust
use charon_physics::sim::{
    material::CellData,
    physics::ParticleContext,
    rules::{MaterialDefinition, Movement},
};

fn condense(context: &mut ParticleContext<'_>, _rng: &mut dyn rand::Rng) {
    let cell = context.cell();
    if cell.extra == 0 {
        context.replace(CellData::water(0));
    } else {
        context.replace(CellData::new(cell.kind, cell.extra - 1));
    }
}

let definition = MaterialDefinition::new(
    "Condensing cloud", Movement::Gas, 0.7, [220, 220, 220, 255],
).with_update(condense);
```

登録したこの物質は、セルの `extra` を寿命として減らし、0で水へ変わります。初期値は生成時に `CellData::new(material_id, life)` で設定します。コールバックが標準更新を置き換えるため、移動も必要ならコールバック内で実装します。

`ParticleContext` には次の操作があります。

- `cell()` / `position()`：現在のセルと座標。
- `neighbor()`：周囲のセルを読む。
- `material_named()` / `properties()`：登録した物質や密度・移動特性を調べる。
- `try_move()`：1セル以内で、境界と衝突を確認して移動する。
- `replace()` / `transform_neighbor()`：自分や周囲を別の登録物質へ変える。
- `spawn()`：周囲の空セルへ生成する。

コールバックは既定で次tickも実行対象を維持します。安定状態で休止させたい場合は `sleep()` を呼べます。これらの変更は更新フラグと表面キャッシュを更新します。外部コリジョンのBlockは周囲への変換対象にできません。乱数はSimulationのseedに従うRNGを使います。ユーザー関数自身が作る物質量や処理時間は、その実装の責務です。

## ID・保存・制約

物質IDは1バイトで、1ワールド最大256物質です。標準プリセット10個に加えて246個まで登録できます。`CellData` はIDと8ビットの追加状態の合計2バイトです。温度・速度・大きな状態を追加する場合は、別配列やPlugin側の状態で持つ方針です。

IDは登録順に割り当て、そのRegistryの中で使います。別のRegistryのIDをそのまま持ち込まず、名前や保存した定義一覧で対応を取ってください。登録の削除・IDの再利用は提供しません。同じ名前、空名、不正な密度、上限超過は明示的なエラーです。

標準プリセットの燃焼・溶解などの反応は標準物質間のものです。新しい物質の化学反応を自動で推定せず、独自更新で定義します。

今回の定義はRust APIで構築します。アセットファイルからの定義ロード、スクリプト言語、動的ライブラリのロードはM2には含めません。

## 改名と既存APIからの移行

crate名は `charon_physics` です。依存設定と `use` の物理ライブラリ名を更新してください。CoreとFalling Sand Pluginは独立したままです。

`Cell` は固定enumからRegistry内の物質IDへ変わりました。IDの数値が必要なら `id()`、名前・色・実際の物性が必要ならRegistryを参照します。標準の `Cell::Sand` などの定数と `CellData::sand()` などは維持しています。独自物質の登録・生成には、返された `MaterialId` と `CellData::new()` を使用してください。
