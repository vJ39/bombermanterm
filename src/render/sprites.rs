//! ドット絵スプライト定義。
//!
//! 各スプライトは16x16の文字アート(`&[&str]`)+パレット(文字→色)で定義し、
//! `Sprite::from_art` でパレットインデックス相当の色配列に変換する。
//! `.` は透過(下地を透かす)として扱う。本家ボンバーマンのシルエット
//! (丸い頭のプレイヤー・バルーン状の敵・陰影付きブロック)を意識しつつ、
//! 配色・輪郭は完全にオリジナルとする(本家の画像は一切参照・使用しない)。
//! 各行の幅を丸みのある輪郭(カプセル状)で変化させることで、頭でっかちの
//! 寸胴シルエットを表現している(旧8x8では解像度不足で「四角い塊に目が2つ」
//! にしか見えなかった)。

use ratatui::style::Color;

/// スプライトの一辺のピクセル数。
/// 8x8では丸み・手足を表現しきれず「四角い塊に目が2つ」にしか見えなかったため、
/// 16x16に上げてシルエットの解像度を確保する。
pub const SPRITE_SIZE: usize = 16;

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

/// 壁(破壊不可)。石/コンクリート調の陰影ブロック。レンガの目地(横線)を
/// 2本入れて、単なる無地の四角ではなく積まれた石材に見えるようにする。
pub fn wall_sprite() -> Sprite {
    const OUTLINE: Color = Color::Rgb(38, 40, 46);
    const HILITE: Color = Color::Rgb(215, 218, 226);
    const BODY: Color = Color::Rgb(150, 154, 164);
    Sprite::from_art(
        [
            "1111111111111111",
            "1222222222222221",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1111111111111111",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1111111111111111",
            "1333333333333331",
            "1333333333333331",
            "1222222222222221",
            "1111111111111111",
        ],
        &[('1', OUTLINE), ('2', HILITE), ('3', BODY)],
    )
}

/// 壊せるブロック。木箱調。横に渡した板(暗い木目の帯)を2本入れて
/// 木箱らしい構造を表現する。
pub fn block_sprite() -> Sprite {
    const OUTLINE: Color = Color::Rgb(64, 36, 14);
    const WOOD: Color = Color::Rgb(216, 152, 84);
    const WOOD_LIGHT: Color = Color::Rgb(184, 112, 52);
    const WOOD_DARK: Color = Color::Rgb(120, 68, 26);
    Sprite::from_art(
        [
            "1111111111111111",
            "1222222222222221",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1444444444444441",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1444444444444441",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1333333333333331",
            "1222222222222221",
            "1111111111111111",
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
        // 顔(ほぼ純白)と輝度が近すぎると頭と体の境目が消えるため、
        // 寒色寄りの薄いグレーにして「白いボンバーマン」のイメージを保ちつつ
        // コントラストを確保する。
        PlayerColor::White => (Color::Rgb(188, 202, 218), Color::Rgb(222, 232, 244)),
        PlayerColor::Black => (Color::Rgb(50, 52, 58), Color::Rgb(90, 92, 100)),
        PlayerColor::Red => (Color::Rgb(210, 40, 40), Color::Rgb(240, 90, 90)),
        PlayerColor::Blue => (Color::Rgb(40, 90, 210), Color::Rgb(90, 140, 240)),
    };
    player_sprite_with_suit(suit, suit_light)
}

/// スーツの色を直接指定してプレイヤースプライトを組み立てる。
/// 無敵モード中の点滅表現(固定4色に無い色をその場で使いたい場合)向けの入口。
///
/// 本家ボンバーマンの「丸い頭+離れた黒目+頭でっかちの寸胴シルエット」を
/// 16x16で再現する(8x8では丸み・手足を表現しきれず「四角い塊」にしか
/// 見えなかったため解像度を上げた)。各行の幅を丸みのある輪郭(カプセル状)で
/// 変化させ、頭部(row0-8)→肩(row9)→胴(row10-12)→腰(row13)→脚(row14-15)
/// とプロポーションを付ける。
pub fn player_sprite_with_suit(suit: Color, suit_light: Color) -> Sprite {
    const OUTLINE: Color = Color::Rgb(18, 18, 22);
    const FACE: Color = Color::Rgb(248, 247, 245);
    const EYE: Color = Color::Rgb(16, 18, 26);
    Sprite::from_art(
        [
            ".....122221.....",
            "...1222222221...",
            "..122222222221..",
            ".1223322223321..",
            ".1223322223321..",
            ".1222222222221..",
            ".1222222222221..",
            "..122222222221..",
            "...1222222221...",
            "..144444444441..",
            ".14455444455441.",
            ".14454444454441.",
            ".14444444444441.",
            "..144444444441..",
            "...111....111...",
            "..1111....1111..",
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

/// バルーン状の一体シルエット(頭と体の間に区切りの輪郭を入れない)。
/// 目は列2/5にはっきり離して置き、単色の体の上でも視認できる白目にする。
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
            ".....122221.....",
            "...1222222221...",
            "..122222222221..",
            ".1223322223321..",
            ".1223322223321..",
            ".1222222222221..",
            ".1222222222221..",
            ".1222222222221..",
            ".1222222222221..",
            ".1222222222221..",
            "..122222222221..",
            "..122222222221..",
            "..12222222221...",
            "...1222222221...",
            "....12222221....",
            "....444..444....",
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
            ".....3..........",
            "......3.........",
            ".......3........",
            "....12222221....",
            "..12222222221...",
            "..124222222221..",
            ".1242222222221..",
            ".12422222222221.",
            ".12222222222221.",
            ".12222222222221.",
            ".12222222222221.",
            ".12222222222221.",
            ".1222222222221..",
            "..122222222221..",
            "..12222222221...",
            "....12222221....",
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
            ".......33.......",
            "......3333......",
            "...3..3333..3...",
            "..333.2222.333..",
            "...3222222223...",
            "....22111122....",
            ".33221111112233.",
            "3332211111122333",
            "3332211111122333",
            ".33221111112233.",
            "....22111122....",
            "...3222222223...",
            "..333.2222.333..",
            "...3..3333..3...",
            "......3333......",
            ".......33.......",
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
            "................",
            "................",
            ".....122221.....",
            "...1222222221...",
            "..122222222221..",
            ".1222333333221..",
            ".1222333333221..",
            ".1222333333221..",
            ".1222333333221..",
            ".1222222222221..",
            ".1222222222221..",
            "..122222222221..",
            "...1222222221...",
            ".....122221.....",
            "................",
            "................",
        ],
        &[('1', OUTLINE), ('2', SHELL), ('3', mark)],
    )
}

