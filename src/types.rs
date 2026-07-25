//! 共通の型契約置き場。
//!
//! この契約以外の型を増やす場合は、追加箇所のdocコメントに
//! "CONTRACT CHANGE:" と明記すること。

/// マップ・移動座標の共通表現。(row, col) の順。
pub type Coord = (i32, i32);

/// プレイヤー・敵の移動方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// 入力から変換される、その tick で行うアクション。
///
/// CONTRACT CHANGE: `ToggleGodMode` を追加。通常操作には無いキーに割り当てる
/// 隠しコマンドで、押すたびに強制無敵モードのON/OFFを切り替える。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Move(Direction),
    PlaceBomb,
    Pause,
    Quit,
    ToggleGodMode,
    None,
}

/// マップ上の1マスの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Empty,
    Wall,
    Block,
    ItemTile(ItemKind),
}

/// アイテムの種類。
///
/// CONTRACT CHANGE: `Invincible` を追加。取得後一定時間、爆風・敵接触で
/// 死亡しなくなる無敵モードを付与する(本家の定番アイテムに寄せた追加)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Power,
    BombUp,
    SpeedUp,
    Invincible,
}

/// 単発の効果音。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEffect {
    PlaceBomb,
    Explosion,
    ItemGet,
    Death,
    StageClear,
}

/// ループ再生されるBGM。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bgm {
    Title,
    Stage,
    Clear,
    GameOver,
}

/// ゲーム全体の画面状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Title,
    Playing,
    Cleared,
    GameOver,
}
