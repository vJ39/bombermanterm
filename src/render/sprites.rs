//! ドット絵スプライト定義。
//!
//! 各スプライトは8x8の文字アート(`&[&str]`)+パレット(文字→色)で定義し、
//! `Sprite::from_art` でパレットインデックス相当の色配列に変換する。
//! `.` は透過(下地を透かす)として扱う。本家ボンバーマンのシルエット
//! (丸い頭のプレイヤー・バルーン状の敵・陰影付きブロック)を意識しつつ、
//! 配色・輪郭は完全にオリジナルとする(本家の画像は一切参照・使用しない)。

use ratatui::style::Color;

/// スプライトの一辺のピクセル数。
pub const SPRITE_SIZE: usize = 8;

/// パレットインデックス方式で保持する固定サイズスプライト。
/// `None` は透過(そのマスの下地=タイル背景色を透かす)。
pub struct Sprite {
    pub pixels: [[Option<Color>; SPRITE_SIZE]; SPRITE_SIZE],
}

impl Sprite {
    /// 8行の文字アートとパレット(文字→色)から `Sprite` を組み立てる。
    /// 各行はちょうど `SPRITE_SIZE` 文字である必要がある(アサーションで検証)。
    fn from_art(art: [&str; SPRITE_SIZE], palette: &[(char, Color)]) -> Self {
        let mut pixels = [[None; SPRITE_SIZE]; SPRITE_SIZE];
        for (y, row) in art.iter().enumerate() {
            let chars: Vec<char> = row.chars().collect();
            assert_eq!(
                chars.len(),
                SPRITE_SIZE,
                "sprite row must be exactly {SPRITE_SIZE} chars: {row:?}"
            );
            for (x, &c) in chars.iter().enumerate() {
                pixels[y][x] = palette
                    .iter()
                    .find(|(pc, _)| *pc == c)
                    .map(|(_, color)| *color);
            }
        }
        Sprite { pixels }
    }
}

/// 壁(破壊不可)。石/コンクリート調の陰影ブロック。
pub fn wall_sprite() -> Sprite {
    const OUTLINE: Color = Color::Rgb(38, 40, 46);
    const HILITE: Color = Color::Rgb(215, 218, 226);
    const BODY: Color = Color::Rgb(150, 154, 164);
    Sprite::from_art(
        [
            "11111111",
            "12222221",
            "12333321",
            "12333321",
            "12333321",
            "12333321",
            "12222221",
            "11111111",
        ],
        &[('1', OUTLINE), ('2', HILITE), ('3', BODY)],
    )
}

/// 壊せるブロック。木箱/レンガ調。
pub fn block_sprite() -> Sprite {
    const OUTLINE: Color = Color::Rgb(64, 36, 14);
    const WOOD: Color = Color::Rgb(184, 112, 52);
    const WOOD_LIGHT: Color = Color::Rgb(216, 152, 84);
    const WOOD_DARK: Color = Color::Rgb(140, 82, 32);
    Sprite::from_art(
        [
            "11111111",
            "12222221",
            "12343421",
            "12222221",
            "12423241",
            "12222221",
            "12342341",
            "11111111",
        ],
        &[
            ('1', OUTLINE),
            ('2', WOOD),
            ('3', WOOD_LIGHT),
            ('4', WOOD_DARK),
        ],
    )
}

/// プレイヤーの色バリエーション(4人分)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerColor {
    White,
    Black,
    Red,
    Blue,
}

/// プレイヤー。丸い顔+色付きジャンプスーツの本家風シルエット。
pub fn player_sprite(color: PlayerColor) -> Sprite {
    let (suit, suit_light) = match color {
        PlayerColor::White => (Color::Rgb(235, 238, 242), Color::Rgb(255, 255, 255)),
        PlayerColor::Black => (Color::Rgb(50, 52, 58), Color::Rgb(90, 92, 100)),
        PlayerColor::Red => (Color::Rgb(210, 40, 40), Color::Rgb(240, 90, 90)),
        PlayerColor::Blue => (Color::Rgb(40, 90, 210), Color::Rgb(90, 140, 240)),
    };
    player_sprite_with_suit(suit, suit_light)
}

/// スーツの色を直接指定してプレイヤースプライトを組み立てる。
/// 無敵モード中の点滅表現(固定4色に無い色をその場で使いたい場合)向けの入口。
pub fn player_sprite_with_suit(suit: Color, suit_light: Color) -> Sprite {
    const OUTLINE: Color = Color::Rgb(18, 18, 22);
    const FACE: Color = Color::Rgb(244, 224, 196);
    const EYE: Color = Color::Rgb(20, 24, 40);
    Sprite::from_art(
        [
            "..1111..",
            ".1222211",
            "1233321.",
            ".1222211",
            "11111111",
            "1444444.",
            "14555441",
            "1.1..1.1",
        ],
        &[
            ('1', OUTLINE),
            ('2', FACE),
            ('3', EYE),
            ('4', suit),
            ('5', suit_light),
        ],
    )
}

/// CPU敵の種類ごとの色違いバルーン風スプライト。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyColor {
    Magenta,
    Red,
    Blue,
}

pub fn enemy_sprite(color: EnemyColor) -> Sprite {
    const OUTLINE: Color = Color::Rgb(18, 18, 22);
    const EYE: Color = Color::Rgb(250, 250, 250);
    const FOOT: Color = Color::Rgb(30, 30, 34);
    let body = match color {
        EnemyColor::Magenta => Color::Rgb(180, 40, 160),
        EnemyColor::Red => Color::Rgb(200, 40, 40),
        EnemyColor::Blue => Color::Rgb(40, 100, 200),
    };
    Sprite::from_art(
        [
            ".111111.",
            "12222221",
            "12322321",
            "12222221",
            "12233221",
            "12222221",
            "11111111",
            ".4.11.4.",
        ],
        &[('1', OUTLINE), ('2', body), ('3', EYE), ('4', FOOT)],
    )
}

/// ボム。黒い球体+導火線の火花(点滅は呼び出し側で `fuse_hot` を切り替えて表現する)。
pub fn bomb_sprite(fuse_hot: bool) -> Sprite {
    const OUTLINE: Color = Color::Rgb(15, 15, 18);
    const BODY: Color = Color::Rgb(35, 35, 40);
    const HILITE: Color = Color::Rgb(90, 90, 98);
    let fuse = if fuse_hot {
        Color::Rgb(255, 90, 20)
    } else {
        Color::Rgb(200, 160, 40)
    };
    Sprite::from_art(
        [
            "..3.....",
            "...3....",
            "..11111.",
            ".142222.",
            ".122222.",
            ".122222.",
            "..11111.",
            "........",
        ],
        &[('1', OUTLINE), ('2', BODY), ('3', fuse), ('4', HILITE)],
    )
}

/// 爆風。中心が白熱し外側がオレンジ〜赤の炎エフェクト。
pub fn explosion_sprite() -> Sprite {
    const CORE: Color = Color::Rgb(255, 250, 220);
    const MID: Color = Color::Rgb(255, 170, 40);
    const OUTER: Color = Color::Rgb(230, 60, 10);
    Sprite::from_art(
        [
            "3......3",
            ".2....2.",
            "..2..2..",
            "...11...",
            "...11...",
            "..2..2..",
            ".2....2.",
            "3......3",
        ],
        &[('1', CORE), ('2', MID), ('3', OUTER)],
    )
}

/// アイテム。白いカプセルの中央に種類ごとの色付きマークを入れて区別する。
pub fn item_sprite(kind: crate::types::ItemKind) -> Sprite {
    use crate::types::ItemKind;
    const OUTLINE: Color = Color::Rgb(20, 20, 24);
    const SHELL: Color = Color::Rgb(248, 248, 250);
    let mark = match kind {
        ItemKind::Power => Color::Rgb(230, 60, 40),
        ItemKind::BombUp => Color::Rgb(200, 60, 190),
        ItemKind::SpeedUp => Color::Rgb(60, 200, 220),
        ItemKind::Invincible => Color::Rgb(255, 210, 30),
    };
    Sprite::from_art(
        [
            ".111111.",
            "12222221",
            "12233221",
            "12333321",
            "12333321",
            "12233221",
            "12222221",
            ".111111.",
        ],
        &[('1', OUTLINE), ('2', SHELL), ('3', mark)],
    )
}
