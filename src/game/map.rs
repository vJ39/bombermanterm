//! マップ生成・タイル参照。
//!
//! 生成ルール:
//! - 外周1マスは常に破壊不可の `Wall`。
//! - 内部は (行, 列) がともに偶数のマスに破壊不可の `Wall` を格子状に固定配置。
//! - 上記以外の内部マスは、左上のプレイヤー初期位置周辺3マス
//!   ((1,1), (1,2), (2,1)) を除いて約30%の確率で破壊可能な `Block` を配置する。
//! - `destroy_block` は破壊可能な `Block` のみ消去し、約30%の確率でアイテムを
//!   抽選して該当マスに `ItemTile` として残す。それ以外の呼び出しは `None`。

use crate::types::{Coord, ItemKind, Tile};
use serde::{Deserialize, Serialize};

/// 破壊可能ブロックが初期配置される確率。
const BLOCK_SPAWN_RATE: f64 = 0.3;
/// ブロック破壊時にアイテムが出現する確率。
const ITEM_DROP_RATE: f64 = 0.3;

/// CONTRACT CHANGE: ネットワーク対戦でマップごとクライアントへ送るため
/// `serde::Serialize` / `serde::Deserialize` の derive を追加した
/// (非公開フィールド `tiles` も同一クレート内なのでそのまま derive できる)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameMap {
    pub width: usize,
    pub height: usize,
    tiles: Vec<Vec<Tile>>,
}

impl GameMap {
    /// 指定サイズのマップを生成する。
    /// 外周は破壊不可の壁、内部は格子状に破壊不可ブロックを固定配置、
    /// 残りマスの一部にランダムで破壊可能ブロックを配置する。
    pub fn generate(width: usize, height: usize) -> Self {
        let mut tiles = vec![vec![Tile::Empty; width]; height];

        for row in 0..height {
            for col in 0..width {
                let r = row as i32;
                let c = col as i32;

                let is_border =
                    row == 0 || col == 0 || row == height - 1 || col == width - 1;
                let is_fixed_wall = r % 2 == 0 && c % 2 == 0;
                let is_player_start_area =
                    (r, c) == (1, 1) || (r, c) == (1, 2) || (r, c) == (2, 1);

                tiles[row][col] = if is_border || is_fixed_wall {
                    Tile::Wall
                } else if is_player_start_area {
                    Tile::Empty
                } else if rand::random_bool(BLOCK_SPAWN_RATE) {
                    Tile::Block
                } else {
                    Tile::Empty
                };
            }
        }

        Self {
            width,
            height,
            tiles,
        }
    }

    /// pos が範囲内なら (row, col) の usize 添字を返す。
    fn index_of(&self, pos: Coord) -> Option<(usize, usize)> {
        let (row, col) = pos;
        if row < 0 || col < 0 {
            return None;
        }
        let (row, col) = (row as usize, col as usize);
        if row >= self.height || col >= self.width {
            return None;
        }
        Some((row, col))
    }

    /// 指定座標のタイルを返す。範囲外は破壊不可の壁として扱う。
    pub fn tile_at(&self, pos: Coord) -> Tile {
        match self.index_of(pos) {
            Some((row, col)) => self.tiles[row][col],
            None => Tile::Wall,
        }
    }

    /// 指定座標がプレイヤー/敵が進入可能か。
    pub fn is_walkable(&self, pos: Coord) -> bool {
        matches!(self.tile_at(pos), Tile::Empty | Tile::ItemTile(_))
    }

    /// 破壊可能ブロックなら消してアイテム抽選、それ以外はNone。
    ///
    /// ブロックを破壊した結果アイテムを引き当てた場合はそのマスに
    /// `ItemTile` を残して `Some(ItemKind)` を返す。引き当てなかった場合は
    /// マスを `Empty` にして `None` を返す。ブロック以外のマスに対する
    /// 呼び出しは何もせず `None` を返す。
    pub fn destroy_block(&mut self, pos: Coord) -> Option<ItemKind> {
        let (row, col) = self.index_of(pos)?;
        if !matches!(self.tiles[row][col], Tile::Block) {
            return None;
        }

        if rand::random_bool(ITEM_DROP_RATE) {
            let kind = match rand::random_range(0..4) {
                0 => ItemKind::Power,
                1 => ItemKind::BombUp,
                2 => ItemKind::SpeedUp,
                _ => ItemKind::Invincible,
            };
            self.tiles[row][col] = Tile::ItemTile(kind);
            Some(kind)
        } else {
            self.tiles[row][col] = Tile::Empty;
            None
        }
    }

    /// アイテムマスからアイテムを回収する。
    ///
    /// CONTRACT CHANGE: 契約に無い追加メソッド。プレイヤーがアイテムを
    /// 拾った際にマップ側の `ItemTile` を消費(`Empty` に戻す)する手段が
    /// 契約上のメソッド一覧(`generate`/`tile_at`/`is_walkable`/`destroy_block`)
    /// に存在しなかったため、Integrateフェーズ(`state.rs`)実装時に追加した。
    /// 既存メソッドのシグネチャ変更は無い、純粋な追加のみ。
    /// 対象マスが `ItemTile` ならそのマスを `Empty` に戻して `Some(ItemKind)` を、
    /// それ以外(範囲外・Empty・Wall・Block)は何もせず `None` を返す。
    pub fn take_item(&mut self, pos: Coord) -> Option<ItemKind> {
        let (row, col) = self.index_of(pos)?;
        if let Tile::ItemTile(kind) = self.tiles[row][col] {
            self.tiles[row][col] = Tile::Empty;
            Some(kind)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_is_wall() {
        let map = GameMap::generate(13, 11);
        for col in 0..map.width {
            assert_eq!(map.tile_at((0, col as i32)), Tile::Wall);
            assert_eq!(map.tile_at((map.height as i32 - 1, col as i32)), Tile::Wall);
        }
        for row in 0..map.height {
            assert_eq!(map.tile_at((row as i32, 0)), Tile::Wall);
            assert_eq!(map.tile_at((row as i32, map.width as i32 - 1)), Tile::Wall);
        }
    }

    #[test]
    fn fixed_grid_walls_are_placed() {
        let map = GameMap::generate(13, 11);
        assert_eq!(map.tile_at((2, 2)), Tile::Wall);
        assert_eq!(map.tile_at((4, 6)), Tile::Wall);
    }

    #[test]
    fn player_start_area_is_clear() {
        let map = GameMap::generate(13, 11);
        assert_eq!(map.tile_at((1, 1)), Tile::Empty);
        assert_eq!(map.tile_at((1, 2)), Tile::Empty);
        assert_eq!(map.tile_at((2, 1)), Tile::Empty);
    }

    #[test]
    fn out_of_bounds_is_wall_and_not_walkable() {
        let map = GameMap::generate(13, 11);
        assert_eq!(map.tile_at((-1, 0)), Tile::Wall);
        assert_eq!(map.tile_at((100, 100)), Tile::Wall);
        assert!(!map.is_walkable((-1, 0)));
    }

    #[test]
    fn destroy_block_only_affects_blocks() {
        let mut map = GameMap::generate(13, 11);
        // 壁は破壊できない。
        assert_eq!(map.destroy_block((0, 0)), None);
        assert_eq!(map.tile_at((0, 0)), Tile::Wall);

        // 空きマスも破壊できない。
        assert_eq!(map.destroy_block((1, 1)), None);
        assert_eq!(map.tile_at((1, 1)), Tile::Empty);
    }

    #[test]
    fn destroy_block_removes_block_and_may_drop_item() {
        // ブロックが見つかるまでマップを何度か生成して検証する。
        for _ in 0..50 {
            let mut map = GameMap::generate(13, 11);
            let mut found = false;
            'outer: for row in 0..map.height {
                for col in 0..map.width {
                    if map.tile_at((row as i32, col as i32)) == Tile::Block {
                        let pos = (row as i32, col as i32);
                        let result = map.destroy_block(pos);
                        match result {
                            Some(kind) => {
                                assert_eq!(map.tile_at(pos), Tile::ItemTile(kind));
                            }
                            None => {
                                assert_eq!(map.tile_at(pos), Tile::Empty);
                            }
                        }
                        assert!(map.is_walkable(pos));
                        found = true;
                        break 'outer;
                    }
                }
            }
            if found {
                return;
            }
        }
        panic!("no Block tile generated across 50 attempts");
    }

    #[test]
    fn take_item_consumes_item_tile_only() {
        let mut map = GameMap::generate(13, 11);

        // 壁・空きマスからは何も取れない。
        assert_eq!(map.take_item((0, 0)), None);
        assert_eq!(map.take_item((1, 1)), None);
        assert_eq!(map.take_item((-1, -1)), None);

        // ブロックを破壊してアイテムが出るまで繰り返し、出たら回収できることを確認する。
        for _ in 0..200 {
            let mut fresh = GameMap::generate(13, 11);
            let mut done = false;
            'outer: for row in 0..fresh.height {
                for col in 0..fresh.width {
                    let pos = (row as i32, col as i32);
                    if fresh.tile_at(pos) == Tile::Block
                        && let Some(kind) = fresh.destroy_block(pos)
                    {
                        assert_eq!(fresh.take_item(pos), Some(kind));
                        assert_eq!(fresh.tile_at(pos), Tile::Empty);
                        // 二度目の回収は無し。
                        assert_eq!(fresh.take_item(pos), None);
                        done = true;
                        break 'outer;
                    }
                }
            }
            if done {
                return;
            }
        }
        panic!("no item drop across 200 attempts");
    }
}
