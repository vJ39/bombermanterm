//! プレイヤー・敵・ボム・爆風のエンティティ定義。
//!
//! Player/Enemy/Bomb のコンストラクタ、Enemy::decide_move、
//! explosion_cells (爆風範囲計算) を実装するフェーズ。

use crate::game::map::GameMap;
use crate::types::{Coord, Direction, Tile};
use rand::RngExt;

pub struct Player {
    pub pos: Coord,
    pub power: u32,
    pub bomb_capacity: u32,
    pub speed: f32,
    pub alive: bool,
    /// 無敵モードの残り秒数。0より大きい間は爆風・敵接触で死亡しない。
    /// CONTRACT CHANGE: `Invincible` アイテム対応のため追加したフィールド。
    pub invincible_remaining: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyKind {
    Wander,
    Chaser,
    Avoider,
}

pub struct Enemy {
    pub pos: Coord,
    pub kind: EnemyKind,
    pub alive: bool,
}

pub struct Bomb {
    pub pos: Coord,
    pub owner_is_player: bool,
    pub timer: f32,
    pub power: u32,
}

pub struct Explosion {
    pub cells: Vec<Coord>,
    pub remaining: f32,
}

/// ボムの初期タイマー(秒)。仕様に明記が無いためここで既定値を持つ。
const DEFAULT_BOMB_TIMER: f32 = 3.0;

/// 爆風が画面に残る時間(秒)。
const DEFAULT_EXPLOSION_DURATION: f32 = 0.5;

/// 全方向を列挙するための固定配列。
const ALL_DIRECTIONS: [Direction; 4] = [
    Direction::Up,
    Direction::Down,
    Direction::Left,
    Direction::Right,
];

/// 方向を (row, col) の差分に変換する。
fn direction_delta(dir: Direction) -> Coord {
    match dir {
        Direction::Up => (-1, 0),
        Direction::Down => (1, 0),
        Direction::Left => (0, -1),
        Direction::Right => (0, 1),
    }
}

/// pos から dir 方向に1マス進んだ座標を返す。
fn step(pos: Coord, dir: Direction) -> Coord {
    let (dr, dc) = direction_delta(dir);
    (pos.0 + dr, pos.1 + dc)
}

/// マンハッタン距離。
fn manhattan(a: Coord, b: Coord) -> i32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}

/// candidates の中で、target に最も近づく方向を返す。
fn closest_direction(candidates: &[Direction], from: Coord, target: Coord) -> Option<Direction> {
    candidates
        .iter()
        .copied()
        .min_by_key(|&d| manhattan(step(from, d), target))
}

/// candidates の中で、target から最も遠ざかる方向を返す。
fn farthest_direction(candidates: &[Direction], from: Coord, target: Coord) -> Option<Direction> {
    candidates
        .iter()
        .copied()
        .max_by_key(|&d| manhattan(step(from, d), target))
}

/// candidates からランダムに1方向を選ぶ。空なら None。
fn random_direction(candidates: &[Direction]) -> Option<Direction> {
    if candidates.is_empty() {
        return None;
    }
    let idx = rand::rng().random_range(0..candidates.len());
    Some(candidates[idx])
}

impl Player {
    pub fn new(pos: Coord) -> Self {
        Self {
            pos,
            power: 1,
            bomb_capacity: 1,
            speed: 1.0,
            alive: true,
            invincible_remaining: 0.0,
        }
    }

    /// 無敵モード中か。
    pub fn is_invincible(&self) -> bool {
        self.invincible_remaining > 0.0
    }
}

impl Enemy {
    pub fn new(pos: Coord, kind: EnemyKind) -> Self {
        Self {
            pos,
            kind,
            alive: true,
        }
    }

    /// マップとプレイヤー位置から次の移動方向を決定する。
    ///
    /// - Wander: 進入可能な方向からランダムに選ぶ。
    /// - Chaser: 進入可能な方向のうち、プレイヤーに最も近づくものを選ぶ。
    /// - Avoider: 進入可能な方向のうち、プレイヤーから最も遠ざかるものを選ぶ。
    ///
    /// 進入可能な方向が一つも無い場合(行き止まり)は、全方向からランダムに
    /// フォールバックする(呼び出し側で is_walkable を再確認する想定)。
    pub fn decide_move(&self, map: &GameMap, player_pos: Coord) -> Direction {
        let walkable: Vec<Direction> = ALL_DIRECTIONS
            .iter()
            .copied()
            .filter(|&d| map.is_walkable(step(self.pos, d)))
            .collect();

        let chosen = match self.kind {
            EnemyKind::Wander => random_direction(&walkable),
            EnemyKind::Chaser => closest_direction(&walkable, self.pos, player_pos),
            EnemyKind::Avoider => farthest_direction(&walkable, self.pos, player_pos),
        };

        chosen.unwrap_or_else(|| {
            random_direction(&ALL_DIRECTIONS).expect("ALL_DIRECTIONS is never empty")
        })
    }
}

impl Bomb {
    pub fn new(pos: Coord, power: u32, owner_is_player: bool) -> Self {
        Self {
            pos,
            owner_is_player,
            timer: DEFAULT_BOMB_TIMER,
            power,
        }
    }
}

impl Explosion {
    /// 指定セル群から爆風を生成する。remaining は表示継続時間の既定値。
    ///
    /// 契約に `Explosion::new` は無いが、Integrate フェーズ (state.rs) が
    /// `explosion_cells` の結果から `Explosion` を組み立てる際の補助として
    /// 用意する最小限のヘルパー。
    /// CONTRACT CHANGE: 契約にない関連関数 `Explosion::new` を追加。
    /// フィールドが private ではなく直接構築も可能なため、必須ではなく
    /// 呼び出し側の利便性のためだけに用意した補助。
    pub fn new(cells: Vec<Coord>) -> Self {
        Self {
            cells,
            remaining: DEFAULT_EXPLOSION_DURATION,
        }
    }
}

/// 爆風範囲計算。
///
/// 原点セルを含め、上下左右の4方向へ `power` マスずつ爆風を伸ばす。
/// 各方向は次のルールで止まる:
/// - `Tile::Wall` (破壊不可壁) に当たったら、その手前までで止まる
///   (壁のマス自体は爆風に含まない)。
/// - `Tile::Block` (破壊可能ブロック) に当たったら、そのブロックのマスまでは
///   爆風に含め、その先へは伸ばさない(ブロックは爆風を止める)。
/// - `Tile::Empty` / `Tile::ItemTile` は爆風が通過し、そのセルは爆風に含まれる。
pub fn explosion_cells(origin: Coord, power: u32, map: &GameMap) -> Vec<Coord> {
    let mut cells = vec![origin];

    for dir in ALL_DIRECTIONS {
        let mut pos = origin;
        for _ in 0..power {
            pos = step(pos, dir);
            match map.tile_at(pos) {
                Tile::Wall => break,
                Tile::Block => {
                    cells.push(pos);
                    break;
                }
                Tile::Empty | Tile::ItemTile(_) => {
                    cells.push(pos);
                }
            }
        }
    }

    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: GameMap::generate / tile_at / is_walkable はまだ todo!() (別フェーズ担当)
    // のため、GameMap を使う explosion_cells / decide_move の結合テストはここでは書けない。
    // ここでは GameMap に依存しない純粋ヘルパーのみ検証する。

    #[test]
    fn direction_delta_matches_expected_offsets() {
        assert_eq!(direction_delta(Direction::Up), (-1, 0));
        assert_eq!(direction_delta(Direction::Down), (1, 0));
        assert_eq!(direction_delta(Direction::Left), (0, -1));
        assert_eq!(direction_delta(Direction::Right), (0, 1));
    }

    #[test]
    fn step_moves_one_cell_in_direction() {
        assert_eq!(step((5, 5), Direction::Up), (4, 5));
        assert_eq!(step((5, 5), Direction::Down), (6, 5));
        assert_eq!(step((5, 5), Direction::Left), (5, 4));
        assert_eq!(step((5, 5), Direction::Right), (5, 6));
    }

    #[test]
    fn manhattan_distance_is_correct() {
        assert_eq!(manhattan((0, 0), (3, 4)), 7);
        assert_eq!(manhattan((2, 2), (2, 2)), 0);
    }

    #[test]
    fn closest_direction_picks_the_move_that_reduces_distance() {
        // player is directly below -> Down should be closest.
        let candidates = [
            Direction::Up,
            Direction::Down,
            Direction::Left,
            Direction::Right,
        ];
        let from = (0, 0);
        let target = (5, 0);
        assert_eq!(
            closest_direction(&candidates, from, target),
            Some(Direction::Down)
        );
    }

    #[test]
    fn farthest_direction_picks_the_move_that_increases_distance() {
        // Restricted to an opposing pair so the result is unambiguous:
        // with all 4 cardinal directions the max distance is always tied
        // between at least two directions (Manhattan distance geometry),
        // so this checks the underlying comparison logic without relying
        // on max_by_key's tie-break rule.
        let candidates = [Direction::Up, Direction::Down];
        let from = (0, 0);
        let target = (5, 0);
        assert_eq!(
            farthest_direction(&candidates, from, target),
            Some(Direction::Up)
        );
    }

    #[test]
    fn closest_and_farthest_direction_return_none_for_empty_candidates() {
        assert_eq!(closest_direction(&[], (0, 0), (1, 1)), None);
        assert_eq!(farthest_direction(&[], (0, 0), (1, 1)), None);
    }

    #[test]
    fn random_direction_returns_none_for_empty_slice() {
        assert_eq!(random_direction(&[]), None);
    }

    #[test]
    fn random_direction_only_returns_candidates_from_input() {
        let candidates = [Direction::Left, Direction::Right];
        for _ in 0..50 {
            let picked = random_direction(&candidates).expect("non-empty candidates");
            assert!(candidates.contains(&picked));
        }
    }

    #[test]
    fn player_new_has_sane_defaults() {
        let player = Player::new((1, 1));
        assert_eq!(player.pos, (1, 1));
        assert!(player.alive);
        assert!(player.power >= 1);
        assert!(player.bomb_capacity >= 1);
        assert!(player.speed > 0.0);
    }

    #[test]
    fn enemy_new_is_alive_and_keeps_kind() {
        let enemy = Enemy::new((2, 3), EnemyKind::Chaser);
        assert_eq!(enemy.pos, (2, 3));
        assert_eq!(enemy.kind, EnemyKind::Chaser);
        assert!(enemy.alive);
    }

    #[test]
    fn bomb_new_keeps_given_fields_and_has_positive_timer() {
        let bomb = Bomb::new((4, 4), 2, true);
        assert_eq!(bomb.pos, (4, 4));
        assert_eq!(bomb.power, 2);
        assert!(bomb.owner_is_player);
        assert!(bomb.timer > 0.0);
    }

    #[test]
    fn explosion_new_stores_cells_and_has_positive_remaining() {
        let explosion = Explosion::new(vec![(0, 0), (0, 1)]);
        assert_eq!(explosion.cells, vec![(0, 0), (0, 1)]);
        assert!(explosion.remaining > 0.0);
    }

    // 以下は explosion_cells の GameMap 結合テスト。
    // マップ生成契約(border/fixed grid/player start area)が確定しているため
    // GameMap を使う結合テストがここで書けるようになった(GameMap自体は
    // 冒頭の `use super::*;` で既にスコープに入っている)。

    fn opposite(dir: Direction) -> Direction {
        match dir {
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }

    #[test]
    fn explosion_cells_stops_before_wall_border_and_inner() {
        let map = GameMap::generate(13, 11);

        // (2,1) はプレイヤー初期位置周辺として常に Empty が保証されるマス。
        // Left隣の(2,0)は外周壁なので、爆風はそこへ到達しない(手前で止まる)。
        let border_cells = explosion_cells((2, 1), 1, &map);
        assert!(border_cells.contains(&(2, 1)));
        assert!(!border_cells.contains(&(2, 0)));

        // (2,2) は行・列ともに偶数なので常に固定の破壊不可壁。
        // 十分な power を与えても、壁そのものにも、その先にも爆風は届かない。
        let inner_cells = explosion_cells((2, 1), 5, &map);
        assert!(!inner_cells.contains(&(2, 2)));
        assert!(!inner_cells.contains(&(2, 3)));
    }

    #[test]
    fn explosion_cells_reaches_block_but_not_beyond_it() {
        // マップ上のどこかにある破壊可能ブロックを探し、その隣の進入可能マスを
        // 爆心地にして、爆風がブロックまでは届き、その先へは伸びないことを確認する。
        for _ in 0..200 {
            let map = GameMap::generate(13, 11);
            for row in 0..map.height as i32 {
                for col in 0..map.width as i32 {
                    let block_pos = (row, col);
                    if map.tile_at(block_pos) != Tile::Block {
                        continue;
                    }
                    for dir in ALL_DIRECTIONS {
                        // origin から dir 方向に進むとちょうど block_pos に着地するように選ぶ。
                        let origin = step(block_pos, opposite(dir));
                        if !map.is_walkable(origin) {
                            continue;
                        }
                        let beyond = step(block_pos, dir);

                        let cells = explosion_cells(origin, 5, &map);
                        assert!(
                            cells.contains(&block_pos),
                            "blast must reach the destructible block itself"
                        );
                        assert!(
                            !cells.contains(&beyond),
                            "blast must not pass through a destructible block"
                        );
                        return;
                    }
                }
            }
        }
        panic!("no Block tile with a walkable neighbor found across 200 attempts");
    }
}
