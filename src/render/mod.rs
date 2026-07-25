//! ratatui描画。
//!
//! `Screen::{Title, Playing, Cleared, GameOver}` の4画面を切り替えて描画する。
//! Playing中は `GameMap` + `Player` + `Enemy` + `Bomb` + `Explosion` をセル単位で描画する。
//! 本家ボンバーマンを彷彿とさせる配色(緑地フィールド・レンガ調の破壊可能ブロック・
//! 灰色の破壊不可ブロック)を意識しつつ、Unicode文字(色付きBox Drawing/Block Elements)と
//! ratatuiのスタイルだけで独自表現する(本家の画像・音源は一切使用しない)。
//! エンティティ(プレイヤー/敵/ボム/アイテム)は草地の市松模様に紛れないよう、
//! 専用の暗い背景([`ENTITY_BG`])を敷いて地肌から浮き上がらせるハイコントラスト構成。
//!
//! 実装メモ:
//! - 端末の1文字セルは縦長になりがちなため、フィールドの論理1マスは横2カラムを使って
//!   見た目を正方形に近づける([`CELL_WIDTH`])。
//! - 同一マスへの重なり優先順位(奥→手前): タイル(背景) < ボム < 敵 < プレイヤー < 爆風。
//!   爆風は演出上もっとも目立たせたいため最前面に描画する。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::game::entities::EnemyKind;
use crate::game::state::GameState;
use crate::types::{Coord, ItemKind, Screen, Tile};

/// フィールド1マスあたりの表示カラム数。
const CELL_WIDTH: usize = 2;

/// 画面全体を `state.screen` に応じて描画する。
pub fn draw(frame: &mut Frame, state: &GameState) {
    match state.screen {
        Screen::Title => draw_title(frame),
        Screen::Playing => draw_playing(frame, state),
        Screen::Cleared => draw_result(frame, state, true),
        Screen::GameOver => draw_result(frame, state, false),
    }
}

/// `area` の中央に `width` x `height` の矩形を配置する。
/// `area` より大きい場合は `area` に収まるようにクランプする。
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

fn draw_title(frame: &mut Frame) {
    let area = frame.area();

    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "╔═══════════════════════════════════════╗",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "║   B O M B E R M A N   T E R M   ║",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "╚═══════════════════════════════════════╝",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "端末で遊ぶオリジナル爆弾アクション",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "[ SPACE ] で ゲーム開始",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "移動: ↑ ↓ ← → / h j k l    設置: Space    ポーズ: p    終了: Esc / q",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(Text::from(lines)).alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

fn draw_playing(frame: &mut Frame, state: &GameState) {
    let area = frame.area();

    let [status_area, field_container] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

    let status = Line::from(vec![
        Span::styled(
            format!(" SCORE {:06} ", state.score),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" LIVES {} ", state.lives),
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(status).alignment(Alignment::Center),
        status_area,
    );

    let field_width = (state.map.width * CELL_WIDTH) as u16 + 2;
    let field_height = state.map.height as u16 + 2;
    let field_area = centered_rect(field_width, field_height, field_container);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" BombermanTerm ");

    let paragraph = Paragraph::new(render_field(state)).block(block);
    frame.render_widget(paragraph, field_area);
}

fn draw_result(frame: &mut Frame, state: &GameState, cleared: bool) {
    let area = frame.area();

    let (headline, headline_color) = if cleared {
        ("STAGE CLEAR!", Color::LightGreen)
    } else {
        ("GAME OVER", Color::LightRed)
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            headline,
            Style::default()
                .fg(headline_color)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("SCORE: {:06}", state.score)),
        Line::from(format!("LIVES: {}", state.lives)),
        Line::from(""),
        Line::from(Span::styled(
            "[ SPACE ] タイトルへ    [ Esc / q ] 終了",
            Style::default().fg(Color::Gray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(headline_color));

    let paragraph = Paragraph::new(Text::from(lines))
        .alignment(Alignment::Center)
        .block(block);

    let box_area = centered_rect(46, 10, area);
    frame.render_widget(paragraph, box_area);
}

/// マップ全マスを [`Text`] へ変換する。各行が1マス列、各マスは [`CELL_WIDTH`] カラム分の
/// `Span` として描画される。
fn render_field(state: &GameState) -> Text<'static> {
    let map = &state.map;
    let mut lines = Vec::with_capacity(map.height);
    for row in 0..map.height as i32 {
        let mut spans = Vec::with_capacity(map.width);
        for col in 0..map.width as i32 {
            spans.push(cell_span(state, (row, col)));
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

/// エンティティ(プレイヤー/敵/ボム)の背景色。草地の市松模様に混ざって
/// 見えづらくなるのを避けるため、地肌よりはっきり暗い専用背景を敷いて
/// 前面に浮き上がらせる(ハイコントラスト化)。
const ENTITY_BG: Color = Color::Rgb(10, 10, 14);

/// 1マス分の見た目を決定する。爆風 > プレイヤー > 敵 > ボム > タイル地肌 の優先順位で
/// 最前面のものを描画する。
fn cell_span(state: &GameState, pos: Coord) -> Span<'static> {
    if state
        .explosions
        .iter()
        .any(|explosion| explosion.cells.contains(&pos))
    {
        return Span::styled(
            "**".to_string(),
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(255, 80, 0))
                .add_modifier(Modifier::BOLD),
        );
    }

    if state.player.alive && state.player.pos == pos {
        let fg = player_fg(&state.player);
        return Span::styled(
            " @".to_string(),
            Style::default()
                .fg(fg)
                .bg(ENTITY_BG)
                .add_modifier(Modifier::BOLD),
        );
    }

    if let Some(enemy) = state
        .enemies
        .iter()
        .find(|enemy| enemy.alive && enemy.pos == pos)
    {
        let (glyph, color) = enemy_glyph(enemy.kind);
        return Span::styled(
            format!(" {glyph}"),
            Style::default()
                .fg(color)
                .bg(ENTITY_BG)
                .add_modifier(Modifier::BOLD),
        );
    }

    if let Some(bomb) = state.bombs.iter().find(|bomb| bomb.pos == pos) {
        let fuse_hot = bomb.timer < 1.0 && ((bomb.timer * 6.0) as i32) % 2 == 0;
        let fg = if fuse_hot { Color::LightRed } else { Color::White };
        return Span::styled(
            " ●".to_string(),
            Style::default()
                .fg(fg)
                .bg(ENTITY_BG)
                .add_modifier(Modifier::BOLD),
        );
    }

    tile_span(state.map.tile_at(pos), pos)
}

/// プレイヤーの表示色。
/// - 隠しコマンドの強制無敵(`god_mode`)中は、時間に依存しない固定の金色にする
///   (`invincible_remaining` が常に0のままなので、時限アイテムと同じ点滅方式は使えない)。
/// - 時限アイテムによる無敵中は、残り時間に応じて色を切り替えて点滅させる。
///   現在時刻に依存せず `invincible_remaining` の値だけで決定するため、
///   tick間隔が変わっても点滅サイクルは一定になる。
fn player_fg(player: &crate::game::entities::Player) -> Color {
    if player.god_mode {
        return Color::Rgb(255, 215, 0);
    }
    if player.invincible_remaining <= 0.0 {
        return Color::LightYellow;
    }
    const RAINBOW: [Color; 4] = [
        Color::LightYellow,
        Color::LightCyan,
        Color::LightMagenta,
        Color::White,
    ];
    let idx =
        ((player.invincible_remaining * 10.0) as i64).unsigned_abs() as usize % RAINBOW.len();
    RAINBOW[idx]
}

/// エンティティが乗っていないタイル自体の見た目。
///
/// 本家ボンバーマンの「硬い壁=石/コンクリート」「壊せるブロック=木箱/レンガ」の
/// 見分けやすさを、網掛け文字の密度差(硬い壁ほど密)と色味差(壁=無機質なグレー系/
/// ブロック=暖色の茶系)の二重の手がかりで再現する。
fn tile_span(tile: Tile, pos: Coord) -> Span<'static> {
    match tile {
        Tile::Wall => Span::styled(
            "▓▓".to_string(),
            Style::default()
                .fg(Color::Rgb(205, 208, 215))
                .bg(Color::Rgb(72, 76, 88))
                .add_modifier(Modifier::BOLD),
        ),
        Tile::Block => Span::styled(
            "▒▒".to_string(),
            Style::default()
                .fg(Color::Rgb(220, 145, 60))
                .bg(Color::Rgb(96, 56, 24))
                .add_modifier(Modifier::BOLD),
        ),
        Tile::Empty => Span::styled("  ".to_string(), Style::default().bg(grass_bg(pos))),
        Tile::ItemTile(kind) => {
            let (glyph, color) = item_glyph(kind);
            Span::styled(
                format!("{glyph}{glyph}"),
                Style::default()
                    .fg(color)
                    .bg(ENTITY_BG)
                    .add_modifier(Modifier::BOLD),
            )
        }
    }
}

/// 草地の市松模様。地肌のテクスチャがはっきり見える程度にコントラストを広げた2トーン。
fn grass_bg(pos: Coord) -> Color {
    if (pos.0 + pos.1) % 2 == 0 {
        Color::Rgb(8, 90, 28)
    } else {
        Color::Rgb(34, 160, 60)
    }
}

fn item_glyph(kind: ItemKind) -> (char, Color) {
    match kind {
        ItemKind::Power => ('P', Color::LightRed),
        ItemKind::BombUp => ('B', Color::LightMagenta),
        ItemKind::SpeedUp => ('S', Color::LightCyan),
        ItemKind::Invincible => ('I', Color::Rgb(255, 215, 0)),
    }
}

fn enemy_glyph(kind: EnemyKind) -> (char, Color) {
    match kind {
        EnemyKind::Wander => ('W', Color::LightMagenta),
        EnemyKind::Chaser => ('C', Color::LightRed),
        EnemyKind::Avoider => ('A', Color::LightBlue),
    }
}
