//! ratatui描画。
//!
//! `Screen::{Title, Playing, Cleared, GameOver, MatchResult}` の各画面を切り替えて描画する。
//! Playing中のフィールドは論理ピクセルグリッド([`pixel_canvas::PixelCanvas`])に
//! 各マス8x8ドット絵スプライト([`sprites`])を描き込み、ハーフブロック文字`▀`で
//! 疑似2倍解像度の端末セルへ変換する(`aquaterm`の手法を踏襲)。本家ボンバーマンの
//! シルエット(丸い頭のプレイヤー・バルーン状の敵・陰影付きブロック)を意識しつつ、
//! 配色・輪郭は完全にオリジナルとする(本家の画像・音源は一切使用しない)。
//!
//! 実装メモ:
//! - `zoom` は1論理ピクセルを何文字四方で表現するかの表示スケール(`+`/`-`キーで変更)。
//!   ゲームロジックには影響しないUI専用の値なので `GameState` には持たせず、
//!   呼び出し側([`crate::main`])が保持して `draw` に渡す。
//! - 同一マスへの重なり優先順位(奥→手前): タイル(背景) < ボム < 敵 < プレイヤー < 爆風。
//!   爆風は演出上もっとも目立たせたいため最前面に描画する。
//! - `GameState::players` は複数人を持てるが、この描画は1人プレイ向けのままで
//!   先頭のプレイヤー(プレイヤー0)だけを描く。複数人の同時描画は次フェーズで対応する。

mod pixel_canvas;
mod sprites;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::game::entities::EnemyKind;
use crate::game::state::GameState;
use crate::types::{Coord, Screen, Tile};
use pixel_canvas::PixelCanvas;
use sprites::{
    block_sprite, bomb_sprite, enemy_sprite, explosion_sprite, item_sprite, player_sprite,
    player_sprite_with_suit, wall_sprite, EnemyColor, PlayerColor, Sprite, SPRITE_SIZE,
};

/// 表示スケールの既定値・上下限(1論理ピクセルを何文字四方で表現するか)。
pub const DEFAULT_ZOOM: usize = 1;
pub const MIN_ZOOM: usize = 1;
pub const MAX_ZOOM: usize = 3;

/// 画面全体を `state.screen` に応じて描画する。`zoom` はPlaying画面のみで使う。
pub fn draw(frame: &mut Frame, state: &GameState, zoom: usize) {
    match state.screen {
        Screen::Title => draw_title(frame),
        Screen::Playing => draw_playing(frame, state, zoom),
        Screen::Cleared => draw_result(frame, state, true),
        Screen::GameOver => draw_result(frame, state, false),
        Screen::MatchResult(winner) => draw_match_result(frame, winner),
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
            "移動: ↑ ↓ ← → / h j k l    設置: Space    終了: Esc / q    表示拡大縮小: + / -",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(Text::from(lines)).alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

fn draw_playing(frame: &mut Frame, state: &GameState, zoom: usize) {
    let area = frame.area();

    let [status_area, field_container] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

    let status = Line::from(vec![
        Span::styled(
            format!(" SCORE {:06} ", state.score()),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" LIVES {} ", state.lives()),
            Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" x{zoom} "),
            Style::default().fg(Color::Black).bg(Color::Gray),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(status).alignment(Alignment::Center),
        status_area,
    );

    let canvas = render_field_canvas(state);
    let field_lines = canvas.to_lines(zoom);
    // 1行あたりのカラム数はどの行も等しい(全マス分のスパンを敷き詰めているため)ので先頭行から取る。
    let field_cols = field_lines.first().map(|l| l.spans.len()).unwrap_or(0) as u16;
    let field_rows = field_lines.len() as u16;
    let field_area = centered_rect(field_cols + 2, field_rows + 2, field_container);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" BombermanTerm ");

    let paragraph = Paragraph::new(Text::from(field_lines)).block(block);
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
        Line::from(format!("SCORE: {:06}", state.score())),
        Line::from(format!("LIVES: {}", state.lives())),
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

/// 複数プレイヤー対戦の決着画面。`winner` が `None` なら相打ちの引き分け。
fn draw_match_result(frame: &mut Frame, winner: Option<usize>) {
    let area = frame.area();

    let (headline, headline_color) = match winner {
        // プレイヤー番号は内部の添字が0起点なので、表示は1起点にする。
        Some(idx) => (format!("PLAYER {} WINS!", idx + 1), Color::LightGreen),
        None => ("DRAW".to_string(), Color::LightYellow),
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

    let box_area = centered_rect(46, 8, area);
    frame.render_widget(paragraph, box_area);
}

/// フィールド全体を論理ピクセルキャンバスへ描き込む。
/// マスごとに 地肌(市松) → タイル(壁/ブロック/アイテム) → ボム → 敵 → プレイヤー → 爆風
/// の順で重ね描きする(奥から手前)。
fn render_field_canvas(state: &GameState) -> PixelCanvas {
    let map = &state.map;
    let width_px = map.width * SPRITE_SIZE;
    let height_px = map.height * SPRITE_SIZE;
    let mut canvas = PixelCanvas::new(width_px, height_px, Color::Rgb(8, 90, 28));

    for row in 0..map.height as i32 {
        for col in 0..map.width as i32 {
            let pos = (row, col);
            let ox = col as usize * SPRITE_SIZE;
            let oy = row as usize * SPRITE_SIZE;

            fill_grass_bg(&mut canvas, ox, oy, pos);

            match map.tile_at(pos) {
                Tile::Wall => canvas.blit_sprite(ox, oy, &wall_sprite()),
                Tile::Block => canvas.blit_sprite(ox, oy, &block_sprite()),
                Tile::ItemTile(kind) => canvas.blit_sprite(ox, oy, &item_sprite(kind)),
                Tile::Empty => {}
            }

            if let Some(bomb) = state.bombs.iter().find(|bomb| bomb.pos == pos) {
                let fuse_hot = bomb.timer < 1.0 && ((bomb.timer * 6.0) as i32) % 2 == 0;
                canvas.blit_sprite(ox, oy, &bomb_sprite(fuse_hot));
            }

            if let Some(enemy) = state
                .enemies
                .iter()
                .find(|enemy| enemy.alive && enemy.pos == pos)
            {
                canvas.blit_sprite(ox, oy, &enemy_sprite(enemy_color(enemy.kind)));
            }

            // 1人プレイ向けの描画のため、先頭のプレイヤーだけを描く。
            if let Some(player) = state.players.first()
                && player.alive
                && player.pos == pos
            {
                canvas.blit_sprite(ox, oy, &player_sprite_for(player));
            }

            if state
                .explosions
                .iter()
                .any(|explosion| explosion.cells.contains(&pos))
            {
                canvas.blit_sprite(ox, oy, &explosion_sprite());
            }
        }
    }

    canvas
}

/// 8x8マス全体を草地の市松模様(地肌)で塗る。
fn fill_grass_bg(canvas: &mut PixelCanvas, ox: usize, oy: usize, pos: Coord) {
    let color = grass_bg(pos);
    for dy in 0..SPRITE_SIZE {
        for dx in 0..SPRITE_SIZE {
            canvas.set(ox + dx, oy + dy, color);
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

/// プレイヤーのスプライトを状態に応じて選ぶ。
/// - 隠しコマンドの強制無敵(`god_mode`)中は、時間に依存しない固定の金色スーツにする
///   (`invincible_remaining` が常に0のままなので、時限アイテムと同じ点滅方式は使えない)。
/// - 時限アイテムによる無敵中は、残り時間に応じてスーツ色を切り替えて点滅させる。
///   現在時刻に依存せず `invincible_remaining` の値だけで決定するため、
///   tick間隔が変わっても点滅サイクルは一定になる。
/// - 通常時は素の白スーツ(1人プレイの既定色)。
fn player_sprite_for(player: &crate::game::entities::Player) -> Sprite {
    const GOD_SUIT: Color = Color::Rgb(255, 215, 0);
    const GOD_SUIT_LIGHT: Color = Color::Rgb(255, 240, 140);
    const RAINBOW: [(Color, Color); 4] = [
        (Color::Rgb(235, 238, 242), Color::Rgb(255, 255, 255)),
        (Color::Rgb(90, 220, 230), Color::Rgb(160, 245, 250)),
        (Color::Rgb(230, 90, 220), Color::Rgb(250, 160, 245)),
        (Color::Rgb(255, 235, 90), Color::Rgb(255, 250, 170)),
    ];

    if player.god_mode {
        return player_sprite_with_suit(GOD_SUIT, GOD_SUIT_LIGHT);
    }
    if player.invincible_remaining > 0.0 {
        let idx = ((player.invincible_remaining * 10.0) as i64).unsigned_abs() as usize
            % RAINBOW.len();
        let (suit, suit_light) = RAINBOW[idx];
        return player_sprite_with_suit(suit, suit_light);
    }
    player_sprite(PlayerColor::White)
}

fn enemy_color(kind: EnemyKind) -> EnemyColor {
    match kind {
        EnemyKind::Wander => EnemyColor::Magenta,
        EnemyKind::Chaser => EnemyColor::Red,
        EnemyKind::Avoider => EnemyColor::Blue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::state::GameState;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// `state` を仮想端末へ描画し、画面に出た文字を1本の文字列として返す。
    /// 端末を持たない環境でも描画経路がpanicしないことの確認に使う。
    fn rendered_text(state: &GameState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, state, DEFAULT_ZOOM))
            .expect("draw must not fail");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn playing_hud_shows_player_zero_score_and_lives() {
        let mut state = GameState::new();
        state.screen = Screen::Playing;
        state.players[0].score = 1250;
        state.players[0].lives = 2;

        let text = rendered_text(&state);
        assert!(
            text.contains("SCORE 001250"),
            "HUD must show player 0 score"
        );
        assert!(text.contains("LIVES 2"), "HUD must show player 0 lives");
    }

    #[test]
    fn every_screen_renders_without_panicking() {
        let mut state = GameState::new();

        for screen in [
            Screen::Title,
            Screen::Playing,
            Screen::Cleared,
            Screen::GameOver,
        ] {
            state.screen = screen;
            let text = rendered_text(&state);
            assert!(!text.trim().is_empty(), "{screen:?} must draw something");
        }

        assert!(rendered_text(&state).contains("GAME OVER"));
        state.screen = Screen::Cleared;
        assert!(rendered_text(&state).contains("STAGE CLEAR!"));
    }

    #[test]
    fn match_result_screen_shows_the_winner_or_a_draw() {
        let mut state = GameState::new();

        // 表示は1起点にするので、添字1のプレイヤーは "PLAYER 2"。
        state.screen = Screen::MatchResult(Some(1));
        assert!(rendered_text(&state).contains("PLAYER 2 WINS!"));

        state.screen = Screen::MatchResult(None);
        assert!(rendered_text(&state).contains("DRAW"));
    }

    #[test]
    fn playing_field_renders_with_multiple_players_present() {
        // 複数人の同時描画は次フェーズだが、`players` が複数あっても
        // 描画経路がpanicしない(先頭のプレイヤーを描く)ことを確認する。
        let mut state = GameState::new_multiplayer(4);
        state.screen = Screen::Playing;

        let text = rendered_text(&state);
        assert!(text.contains("SCORE 000000"));
    }
}
