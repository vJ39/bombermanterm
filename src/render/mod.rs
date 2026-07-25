//! ratatui描画。
//!
//! `Screen::{Title, Playing, Cleared, GameOver, MatchResult}` の各画面を切り替えて描画する。
//! Playing中のフィールドは論理ピクセルグリッド([`pixel_canvas::PixelCanvas`])に
//! 各マス16x16ドット絵スプライト([`sprites`])を描き込み、ハーフブロック文字`▀`で
//! 疑似2倍解像度の端末セルへ変換する(`aquaterm`の手法を踏襲)。本家ボンバーマンの
//! シルエット(丸い頭のプレイヤー・バルーン状の敵・陰影付きブロック)を意識しつつ、
//! 配色・輪郭は完全にオリジナルとする(本家の画像・音源は一切使用しない)。
//!
//! 実装メモ:
//! - `zoom` は[`ZOOM_MULTIPLIERS`]の添字で、フィールド全体が端末に収まる
//!   最大スケール(自動フィット、[`fit_scale`])に対する追加倍率(`+`/`-`キーで変更)。
//!   ゲームロジックには影響しないUI専用の値なので `GameState` には持たせず、
//!   呼び出し側([`crate::main`])が保持して `draw` に渡す。
//! - 同一マスへの重なり優先順位(奥→手前): タイル(背景) < ボム < 敵 < プレイヤー < 爆風。
//!   爆風は演出上もっとも目立たせたいため最前面に描画する。
//! - `GameState::players` の全員を、添字ごとの色([`PLAYER_COLORS`])で描く。
//!   プレイヤー0の色は従来と同じ白なので、1人プレイの見た目は変わらない。
//! - ネットワーク対戦では「画面を見ている人がどのプレイヤーか」で表示を変えたいので、
//!   [`draw_with_perspective`] に自分のプレイヤー番号を渡せるようにしてある。
//!   ローカル1人プレイは従来どおり [`draw`](自分の番号を指定しない)を使う。

mod intro;
mod menu;
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
// 起動メニューの描画は `menu` サブモジュールに置き、呼び出し側からは
// `draw_intro` 等と同じ `render::` 直下の関数として見えるようにする。
pub(crate) use menu::{draw_host_setup, draw_join_input, draw_mode_select};
use sprites::{
    block_sprite, bomb_sprite, enemy_sprite, explosion_sprite, item_sprite, player_sprite,
    player_sprite_with_suit, wall_sprite, EnemyColor, PlayerColor, Sprite, SPRITE_SIZE,
};

/// 手動ズーム(`+`/`-`キー)の段階。フィールド全体が端末に収まる最大スケール
/// (自動フィット、[`fit_scale`])に対する追加倍率で、絶対倍率ではない。
/// これにより、端末サイズに関わらず「今の表示から拡大/縮小したい」という
/// 操作が一貫して働く。
const ZOOM_MULTIPLIERS: [f32; 5] = [0.5, 0.75, 1.0, 1.5, 2.0];

/// 表示スケールの既定値・上下限([`ZOOM_MULTIPLIERS`] の添字)。既定は1.0倍
/// (自動フィットそのまま)。
pub const DEFAULT_ZOOM: usize = 2;
pub const MIN_ZOOM: usize = 0;
pub const MAX_ZOOM: usize = ZOOM_MULTIPLIERS.len() - 1;

/// プレイヤー添字ごとの色。添字0は従来の1人プレイと同じ白。
const PLAYER_COLORS: [PlayerColor; 4] = [
    PlayerColor::White,
    PlayerColor::Black,
    PlayerColor::Red,
    PlayerColor::Blue,
];

/// 画面全体を `state.screen` に応じて描画する。`zoom` はPlaying画面のみで使う。
///
/// ローカル1人プレイ向けの入口。ネットワーク対戦では
/// [`draw_with_perspective`] に自分のプレイヤー番号を渡す。
pub fn draw(frame: &mut Frame, state: &GameState, zoom: usize) {
    draw_with_perspective(frame, state, zoom, None, None);
}

/// 「画面を見ている人がどのプレイヤーか」を踏まえて描画する。
///
/// `local_player` は `GameState::players` 内の自分の添字。ネットワーク対戦で
/// 4人が同じ盤面を見るため、HUDで自分の色・番号が分かるようにする。
/// `None` はローカル1人プレイ(自分が誰かを示す必要が無い)。
///
/// `host_addr` は自分が待ち受けているアドレス。ホスト(`local_player == Some(0)`)の
/// ロビー画面にだけ出して、参加者へ伝える接続先をTUIの中で確認できるようにする。
pub fn draw_with_perspective(
    frame: &mut Frame,
    state: &GameState,
    zoom: usize,
    local_player: Option<usize>,
    host_addr: Option<&str>,
) {
    match state.screen {
        Screen::Title => draw_title(frame),
        Screen::Lobby {
            connected,
            required,
        } => draw_lobby(frame, connected, required, local_player, host_addr),
        Screen::Playing => draw_playing(frame, state, zoom, local_player),
        Screen::Cleared => draw_result(frame, state, true),
        Screen::GameOver => draw_result(frame, state, false),
        Screen::MatchResult(winner) => draw_match_result(frame, winner),
    }
}

/// サーバーからの最初のスナップショットが届くまでの待ち画面(クライアント専用)。
///
/// クライアントは自分では状態を持たないので、接続直後の1tickだけ
/// 描画するものが無い。その間に出す文字だけの画面。
pub fn draw_connecting(frame: &mut Frame, addr: &str) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "CONNECTED",
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("{addr} のホストと同期しています…")),
        Line::from(""),
        Line::from(Span::styled(
            "[ Esc / q ] 退出",
            Style::default().fg(Color::Gray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let paragraph = Paragraph::new(Text::from(lines))
        .alignment(Alignment::Center)
        .block(block);

    frame.render_widget(paragraph, centered_rect(52, 9, frame.area()));
}

/// 起動時のオンボーディング画面。`GameState` を経由しない独立した画面なので、
/// `crate::main` から直接呼ぶ(`Screen` には含めない、詳細は `render::intro` 参照)。
pub fn draw_intro(frame: &mut Frame) {
    let area = frame.area();
    let canvas = intro::build_canvas();
    let art_lines = canvas.to_lines(1.0);
    let art_cols = art_lines.first().map(|l| l.spans.len()).unwrap_or(0) as u16;
    let art_rows = art_lines.len() as u16;

    let art_area = centered_rect(art_cols, art_rows.saturating_add(2), area);
    frame.render_widget(Paragraph::new(Text::from(art_lines)).alignment(Alignment::Center), art_area);

    let hint_area = Rect::new(
        area.x,
        art_area.y.saturating_add(art_area.height),
        area.width,
        1,
    );
    if hint_area.y < area.y + area.height {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "何かキーを押して開始",
                Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Center),
            hint_area,
        );
    }
}

/// 論理ピクセルサイズ `content_width_px` x `content_height_px` のフィールドが、
/// `available_cols` x `available_rows`(テキスト単位)にちょうど収まる最大の
/// スケールを計算する。ハーフブロック変換で縦2ピクセル=1テキスト行になるため、
/// 縦方向は `available_rows * 2` 論理ピクセル分を使える。
///
/// 引数がゼロ(端末サイズ取得前・空のフィールド等)なら安全に1.0を返す。
fn fit_scale(
    available_cols: u16,
    available_rows: u16,
    content_width_px: usize,
    content_height_px: usize,
) -> f32 {
    if available_cols == 0 || available_rows == 0 || content_width_px == 0 || content_height_px == 0
    {
        return 1.0;
    }
    let by_width = available_cols as f32 / content_width_px as f32;
    let by_height = (available_rows as f32 * 2.0) / content_height_px as f32;
    by_width.min(by_height)
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

/// ネットワーク対戦の参加者待ち画面。
///
/// 凝った演出は置かず、参加人数と開始操作の案内だけを出す。ホスト
/// (`local_player == Some(0)`)には開始キーの案内と自分の待ち受けアドレスを、
/// クライアントにはホスト待ちであることを表示する。
fn draw_lobby(
    frame: &mut Frame,
    connected: usize,
    required: usize,
    local_player: Option<usize>,
    host_addr: Option<&str>,
) {
    let is_host = local_player == Some(0);
    let ready = connected >= required;

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "NETWORK BATTLE - LOBBY",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("参加者 {connected} / {required} 人"),
            Style::default()
                .fg(if ready {
                    Color::LightGreen
                } else {
                    Color::White
                })
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    if let Some(idx) = local_player {
        lines.push(Line::from(Span::styled(
            format!(
                "あなたは PLAYER {}{}",
                idx + 1,
                if is_host { " (ホスト)" } else { "" }
            ),
            Style::default().fg(player_hud_color(idx)),
        )));
        lines.push(Line::from(""));
    }

    // 接続先はホストが参加者へ伝える情報なので、クライアント側には出さない。
    if let (true, Some(addr)) = (is_host, host_addr) {
        lines.push(Line::from(Span::styled(
            format!("接続先: {addr}"),
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }

    let hint = match (is_host, ready) {
        (true, true) => "[ SPACE ] 対戦開始",
        (true, false) => "参加者が揃うまでお待ちください",
        (false, _) => "ホストの開始待ちです",
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[ Esc / q ] 退出",
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title(" BombermanTerm ");
    let paragraph = Paragraph::new(Text::from(lines))
        .alignment(Alignment::Center)
        .block(block);

    frame.render_widget(paragraph, centered_rect(52, 14, frame.area()));
}

fn draw_playing(frame: &mut Frame, state: &GameState, zoom: usize, local_player: Option<usize>) {
    let area = frame.area();

    let [status_area, field_container] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

    let mut spans = vec![
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
    ];

    // 対戦中は、誰が生き残っているか・自分がどのプレイヤーかを出す
    // (4人が同じ盤面を見るため、色と番号の対応が分からないと操作できない)。
    if state.players.len() > 1 {
        for (idx, player) in state.players.iter().enumerate() {
            let you_mark = if local_player == Some(idx) { "(YOU)" } else { "" };
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!(
                    " P{}{} {} ",
                    idx + 1,
                    you_mark,
                    if player.alive { "●" } else { "✕" }
                ),
                if player.alive {
                    Style::default()
                        .fg(player_hud_color(idx))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ));
        }
    }

    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        format!(" x{zoom} "),
        Style::default().fg(Color::Black).bg(Color::Gray),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        status_area,
    );

    let canvas = render_field_canvas(state);
    let content_width_px = state.map.width * SPRITE_SIZE;
    let content_height_px = state.map.height * SPRITE_SIZE;
    // ボーダー(Borders::ALL、上下左右1文字ずつ)の分だけ利用可能領域を狭める。
    let available_cols = field_container.width.saturating_sub(2);
    let available_rows = field_container.height.saturating_sub(2);
    let scale = fit_scale(available_cols, available_rows, content_width_px, content_height_px)
        * ZOOM_MULTIPLIERS[zoom.min(ZOOM_MULTIPLIERS.len() - 1)];
    let field_lines = canvas.to_lines(scale);
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

            // 生存プレイヤーを添字ごとの色で描く。同じマスに複数人が重なった場合は
            // 添字の小さいプレイヤーを手前に描く(重なりは一時的なので優先順位は固定でよい)。
            if let Some((idx, player)) = state
                .players
                .iter()
                .enumerate()
                .find(|(_, player)| player.alive && player.pos == pos)
            {
                canvas.blit_sprite(ox, oy, &player_sprite_for(player, idx));
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
/// - 通常時は添字ごとの色([`PLAYER_COLORS`])。添字0は白なので、1人プレイの
///   見た目は従来と変わらない。
fn player_sprite_for(player: &crate::game::entities::Player, idx: usize) -> Sprite {
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
        // プレイヤー添字ではなく点滅の位相。引数の `idx` と紛れないよう別名にする。
        let phase =
            ((player.invincible_remaining * 10.0) as i64).unsigned_abs() as usize % RAINBOW.len();
        let (suit, suit_light) = RAINBOW[phase];
        return player_sprite_with_suit(suit, suit_light);
    }
    player_sprite(PLAYER_COLORS[idx % PLAYER_COLORS.len()])
}

/// HUDでプレイヤー番号に添えるテキスト色。フィールドのスプライト色
/// ([`PLAYER_COLORS`])と対応させる。黒スーツはそのままでは端末で読みにくいため
/// HUDでは灰色に寄せる。
fn player_hud_color(idx: usize) -> Color {
    match PLAYER_COLORS[idx % PLAYER_COLORS.len()] {
        PlayerColor::White => Color::White,
        PlayerColor::Black => Color::Gray,
        PlayerColor::Red => Color::LightRed,
        PlayerColor::Blue => Color::LightBlue,
    }
}

fn enemy_color(kind: EnemyKind) -> EnemyColor {
    match kind {
        EnemyKind::Wander => EnemyColor::Magenta,
        EnemyKind::Chaser => EnemyColor::Red,
        EnemyKind::Avoider => EnemyColor::Blue,
    }
}

/// テスト用: 端末バッファから拾った文字列に `needle` が現れるか。
/// メニューの描画テスト(`render::menu`)からも使う。
///
/// 全角文字は端末セル2つ分を占め、続きのセルには埋め合わせの空白が入る。
/// そのためセルの文字を素に連結すると日本語は「一 文 字 ず つ」に割れて見え、
/// `str::contains` では一致しない。突き合わせる前に両方から空白を落とす。
#[cfg(test)]
fn contains_text(rendered: &str, needle: &str) -> bool {
    fn squash(text: &str) -> String {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    }
    squash(rendered).contains(&squash(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::state::GameState;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn fit_scale_shrinks_to_fit_a_small_terminal() {
        // 15x13マス(1マス16px)=240x208論理ピクセル。80x24テキストのように
        // 極端に狭い端末では、そのまま(scale=1.0)だと大きくはみ出るはずなので、
        // 1.0未満に縮小される。
        let scale = fit_scale(78, 20, 240, 208);
        assert!(scale < 1.0, "small terminals must shrink below 1.0: {scale}");
        assert!(scale > 0.0);
    }

    #[test]
    fn fit_scale_uses_the_tighter_of_width_or_height() {
        // 横は十分だが縦が厳しいケース: 縦基準の倍率が採用されること。
        let by_width = 1000.0 / 240.0;
        let by_height = (20.0 * 2.0) / 208.0;
        let scale = fit_scale(1000, 20, 240, 208);
        assert!(scale < by_width);
        assert!((scale - by_height).abs() < 1e-4);
    }

    #[test]
    fn fit_scale_falls_back_to_one_when_inputs_are_zero() {
        assert_eq!(fit_scale(0, 20, 240, 208), 1.0);
        assert_eq!(fit_scale(80, 0, 240, 208), 1.0);
        assert_eq!(fit_scale(80, 20, 0, 208), 1.0);
        assert_eq!(fit_scale(80, 20, 240, 0), 1.0);
    }

    #[test]
    fn playing_field_fits_inside_a_small_terminal() {
        // 80x24という一般的な小さめの端末サイズでも、フィールドの描画領域が
        // 画面の外にはみ出さないこと(自動フィットが効いていることの確認)。
        let mut state = GameState::new();
        state.screen = Screen::Playing;

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, &state, DEFAULT_ZOOM))
            .expect("draw must not fail");
        // はみ出ていれば render_widget 内でクリップされるだけでpanicはしないため、
        // ここでは「描画自体が成功すること」に加え、実際に何か描かれていることを見る。
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("BombermanTerm"), "field border must be visible");
    }

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
            Screen::Lobby {
                connected: 1,
                required: 4,
            },
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
        let mut state = GameState::new_multiplayer(4);
        state.screen = Screen::Playing;

        let text = rendered_text(&state);
        assert!(text.contains("SCORE 000000"));
    }

    /// `state` を自分のプレイヤー番号付きで描画し、画面の文字列を返す。
    fn rendered_text_as(state: &GameState, local_player: Option<usize>) -> String {
        rendered_text_as_with_addr(state, local_player, None)
    }

    /// `state` を自分のプレイヤー番号と待ち受けアドレス付きで描画し、画面の文字列を返す。
    fn rendered_text_as_with_addr(
        state: &GameState,
        local_player: Option<usize>,
        host_addr: Option<&str>,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal
            .draw(|frame| {
                draw_with_perspective(frame, state, DEFAULT_ZOOM, local_player, host_addr)
            })
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
    fn versus_hud_lists_every_player_and_marks_the_local_one() {
        let mut state = GameState::new_multiplayer(4);
        state.screen = Screen::Playing;
        state.players[2].alive = false;

        let text = rendered_text_as(&state, Some(1));
        for label in ["P1", "P2(YOU)", "P3", "P4"] {
            assert!(text.contains(label), "HUDに {label} が並ぶこと");
        }
        assert!(text.contains("✕"), "脱落したプレイヤーの印が出ること");
        assert!(text.contains("●"), "生存しているプレイヤーの印が出ること");
    }

    #[test]
    fn single_player_hud_keeps_the_score_and_lives_layout() {
        // 1人プレイでは対戦用のプレイヤー一覧を出さない(従来の表示のまま)。
        let mut state = GameState::new();
        state.screen = Screen::Playing;

        let text = rendered_text(&state);
        assert!(text.contains("SCORE 000000"));
        assert!(text.contains("LIVES 3"));
        assert!(!text.contains("P1"), "1人プレイに対戦HUDは出さない");
    }

    #[test]
    fn each_player_index_gets_its_own_suit_color() {
        let state = GameState::new_multiplayer(4);

        // 16x16スプライトの胴体部分(row12, col7、ハイライトの入らない安定した
        // スーツ本体マス)を代表として色を取り出す。
        let suit_colors: Vec<Color> = (0..state.players.len())
            .map(|idx| {
                player_sprite_for(&state.players[idx], idx).pixels[12][7]
                    .expect("スーツ部分は透過ではないこと")
            })
            .collect();

        assert_eq!(
            suit_colors[0],
            player_sprite(PlayerColor::White).pixels[12][7].expect("suit pixel"),
            "プレイヤー0の色は従来の1人プレイと同じ白のまま"
        );
        for (idx, color) in suit_colors.iter().enumerate() {
            for (other_idx, other) in suit_colors.iter().enumerate() {
                if idx != other_idx {
                    assert_ne!(
                        color, other,
                        "プレイヤー{idx}と{other_idx}は違う色で描き分けること"
                    );
                }
            }
        }
    }

    #[test]
    fn all_living_players_are_drawn_on_the_field() {
        let mut state = GameState::new_multiplayer(4);
        state.screen = Screen::Playing;

        // 同じマップで「全員生存」と「プレイヤー0以外は脱落」を描き比べ、
        // 各プレイヤーのマスに差が出ること(=描かれていること)を確認する。
        let mut only_first = state.clone();
        for player in only_first.players.iter_mut().skip(1) {
            player.alive = false;
        }

        let all_alive = render_field_canvas(&state).to_lines(1.0);
        let solo = render_field_canvas(&only_first).to_lines(1.0);

        for idx in 1..state.players.len() {
            let (row, col) = state.players[idx].pos;
            // 1端末セル=縦2ピクセルなので、そのマスの行範囲は半分になる。
            let line_range =
                (row as usize * SPRITE_SIZE / 2)..((row as usize + 1) * SPRITE_SIZE / 2);
            let col_range = (col as usize * SPRITE_SIZE)..((col as usize + 1) * SPRITE_SIZE);

            let differs = line_range.clone().any(|y| {
                col_range
                    .clone()
                    .any(|x| all_alive[y].spans[x].style != solo[y].spans[x].style)
            });
            assert!(
                differs,
                "プレイヤー{idx}が自分のマス {:?} に描かれていること",
                (row, col)
            );
        }
    }

    #[test]
    fn lobby_shows_the_head_count_and_the_host_start_hint() {
        let mut state = GameState::new_multiplayer(3);
        state.enter_lobby(3);

        // 人数が足りない間はホストにも開始キーを案内しない。
        state.set_lobby_connected(2);
        let waiting = rendered_text_as(&state, Some(0));
        assert!(waiting.contains("2 / 3"), "参加人数が出ること: {waiting}");
        assert!(!waiting.contains("SPACE"), "揃うまでは開始を案内しない");

        // 揃ったらホストにだけ開始キーを案内する。
        state.set_lobby_connected(3);
        let host_view = rendered_text_as(&state, Some(0));
        assert!(host_view.contains("3 / 3"));
        assert!(host_view.contains("SPACE"));
        assert!(host_view.contains("PLAYER 1"), "自分の番号が分かること");

        let client_view = rendered_text_as(&state, Some(2));
        assert!(!client_view.contains("SPACE"), "クライアントは開始できない");
        assert!(client_view.contains("PLAYER 3"));
    }

    #[test]
    fn lobby_shows_the_listening_address_to_the_host_only() {
        let mut state = GameState::new_multiplayer(2);
        state.enter_lobby(2);
        state.set_lobby_connected(1);

        let host_view = rendered_text_as_with_addr(&state, Some(0), Some("192.168.1.10:4321"));
        assert!(
            contains_text(&host_view, "接続先: 192.168.1.10:4321"),
            "ホストのロビーに接続先が出ること: {host_view}"
        );

        // クライアントは接続先を伝える側ではないので出さない。
        let client_view = rendered_text_as_with_addr(&state, Some(1), Some("192.168.1.10:4321"));
        assert!(!client_view.contains("192.168.1.10:4321"));

        // アドレス未指定なら従来どおり接続先の行は出ない。
        assert!(!contains_text(&rendered_text_as(&state, Some(0)), "接続先"));
    }

    #[test]
    fn connecting_screen_shows_the_server_address() {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal
            .draw(|frame| draw_connecting(frame, "127.0.0.1:4321"))
            .expect("draw must not fail");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(text.contains("127.0.0.1:4321"));
    }

}
