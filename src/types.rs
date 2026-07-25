//! 共通の型契約置き場。
//!
//! この契約以外の型を増やす場合は、追加箇所のdocコメントに
//! "CONTRACT CHANGE:" と明記すること。
//!
//! CONTRACT CHANGE: ネットワーク対戦(`crate::net`)でサーバーがゲーム状態を
//! JSONにしてクライアントへ送るため、この共通型すべてに
//! `serde::Serialize` / `serde::Deserialize` の derive を追加した。
//! 既存の型・バリアントの意味は変えていない、derive の追加のみ。

use serde::{Deserialize, Serialize};

/// マップ・移動座標の共通表現。(row, col) の順。
pub type Coord = (i32, i32);

/// プレイヤー・敵の移動方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
/// CONTRACT CHANGE: `ZoomIn`/`ZoomOut` を追加。表示スケール(1論理ピクセルを
/// 何文字四方で表現するか)を切り替えるUI操作。ゲームロジックには影響しないため
/// `GameState::tick` には渡さず、`main.rs` 側でUI状態だけを変更する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Move(Direction),
    PlaceBomb,
    Pause,
    Quit,
    ToggleGodMode,
    ZoomIn,
    ZoomOut,
    None,
}

/// マップ上の1マスの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    Power,
    BombUp,
    SpeedUp,
    Invincible,
}

/// 単発の効果音。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundEffect {
    PlaceBomb,
    Explosion,
    ItemGet,
    Death,
    StageClear,
}

/// ループ再生されるBGM。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bgm {
    Title,
    Stage,
    Clear,
    GameOver,
}

/// ゲーム全体の画面状態。
///
/// CONTRACT CHANGE: `MatchResult` を追加。複数プレイヤー対戦
/// (`GameState::new_multiplayer`)の決着画面で、`Some(index)` は勝者となった
/// プレイヤーの `GameState::players` 内の添字、`None` は全滅による引き分けを表す。
/// 1人プレイ+CPU戦は従来どおり `Cleared` / `GameOver` を使い、この状態にはならない。
///
/// CONTRACT CHANGE: `Lobby` を追加。ネットワーク対戦(`bombermanterm host`)で
/// 参加者が揃うのを待つ画面。`connected` は現在参加している人数(ホスト自身を含む)、
/// `required` は開始に必要な人数。ホスト(プレイヤー0)がSPACEを押すと対戦が始まる
/// (`GameState::tick_multi` 側で `connected >= required` を条件に判定する)。
/// クライアントはこの画面もサーバーから届いた状態として描画するだけなので、
/// ロビー表示のために専用のメッセージを増やす必要はない。
/// ローカル1人プレイはこの状態を経由せず、従来どおり `Title` から始まる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Screen {
    Title,
    Lobby { connected: usize, required: usize },
    Playing,
    Cleared,
    GameOver,
    MatchResult(Option<usize>),
}
