//! 起動時のモード選択メニューの描画。
//!
//! `GameState` を経由しない独立した画面なので `Screen` には含めず、
//! [`crate::menu`] の状態機械から直接呼ばれる(`draw_intro` と同じ扱い)。
//! メニュー3画面の描画は `render/mod.rs` へ足さずここに閉じる。

use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::centered_rect;

/// メニュー枠の幅(文字)。3画面で揃えて、遷移しても枠が動かないようにする。
const BOX_WIDTH: u16 = 60;

/// モード選択の項目ラベル。添字が `crate::menu::MenuState::ModeSelect` の `selected`。
pub(crate) const MODE_LABELS: [&str; 3] = [
    "1人プレイ(CPU対戦)",
    "ホストになる(ネット対戦)",
    "参加する(ネット対戦)",
];

/// モード選択画面。`selected` の項目だけ反転表示する。
pub(crate) fn draw_mode_select(frame: &mut Frame, selected: usize) {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "MODE SELECT",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    for (idx, label) in MODE_LABELS.iter().enumerate() {
        lines.push(Line::from(mode_item(label, idx == selected)));
    }

    lines.push(Line::from(""));
    lines.push(hint("[ ↑ ↓ ] 選択    [ Enter ] 決定    [ Q / Esc ] 終了"));

    render_menu_box(frame, lines);
}

/// ホスト設定画面。対戦人数の増減と開始の案内を出す。
pub(crate) fn draw_host_setup(frame: &mut Frame, players: usize, error: Option<&str>) {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "HOST",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("対戦人数: {players}人 (自分を含む)"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    push_error(&mut lines, error);

    lines.push(Line::from(""));
    lines.push(hint("[ ← → ] 人数変更    [ Enter ] 開始    [ Esc ] 戻る"));

    render_menu_box(frame, lines);
}

/// 参加画面。接続先アドレスの入力欄と、直前の失敗理由(あれば)を出す。
pub(crate) fn draw_join_input(frame: &mut Frame, addr: &str, error: Option<&str>) {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "JOIN",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "ホストのアドレスを入力してください",
            Style::default().fg(Color::Gray),
        )),
        // 末尾のブロックはカーソル位置(入力欄であること)を示す。
        Line::from(Span::styled(
            format!("> {addr}█"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "例: 192.168.1.10:4321",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    push_error(&mut lines, error);

    lines.push(Line::from(""));
    lines.push(hint("[ 文字入力 ] アドレス    [ Backspace ] 1文字削除"));
    lines.push(hint("[ Enter ] 接続    [ Esc ] 戻る"));

    render_menu_box(frame, lines);
}

/// モード選択の1項目。選択中は反転させて、どこに居るかを一目で分かるようにする。
fn mode_item(label: &str, selected: bool) -> Span<'_> {
    if selected {
        Span::styled(
            format!(" ▶ {label} "),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        )
    } else {
        Span::styled(format!("   {label} "), Style::default().fg(Color::Gray))
    }
}

fn hint(text: &str) -> Line<'_> {
    Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
}

/// 失敗の理由を行ごとに積む。
///
/// 枠幅に収まらない行は切り詰められてしまうので、OSのエラー文のような長い
/// 補足は呼び出し側が改行で分けて渡せるようにしてある。
fn push_error<'a>(lines: &mut Vec<Line<'a>>, error: Option<&'a str>) {
    if let Some(error) = error {
        lines.push(Line::from(""));
        lines.extend(error.lines().map(error_line));
    }
}

fn error_line(text: &str) -> Line<'_> {
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
    ))
}

/// メニュー3画面共通の枠。高さは行数に合わせるので、画面ごとに指定しない。
fn render_menu_box(frame: &mut Frame, lines: Vec<Line<'_>>) {
    let height = lines.len() as u16 + 2; // 上下のボーダー分。

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title(" BombermanTerm ");
    let paragraph = Paragraph::new(Text::from(lines))
        .alignment(Alignment::Center)
        .block(block);

    frame.render_widget(paragraph, centered_rect(BOX_WIDTH, height, frame.area()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::contains_text;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// `body` を仮想端末へ描画し、画面に出た文字を1本の文字列として返す。
    fn rendered(body: impl FnOnce(&mut Frame)) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal.draw(body).expect("draw must not fail");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn mode_select_lists_every_mode() {
        // 選択位置が範囲外でもpanicしないこと(状態機械側の不具合で描画が落ちない)。
        for selected in 0..MODE_LABELS.len() + 1 {
            let text = rendered(|frame| draw_mode_select(frame, selected));
            for label in MODE_LABELS {
                assert!(contains_text(&text, label), "{label} が並ぶこと: {text}");
            }
        }
    }

    #[test]
    fn mode_label_count_matches_the_state_machine() {
        assert_eq!(MODE_LABELS.len(), crate::menu::MODE_COUNT);
    }

    #[test]
    fn host_setup_shows_the_player_count_and_an_error() {
        let text = rendered(|frame| draw_host_setup(frame, 3, None));
        assert!(contains_text(&text, "対戦人数: 3人"), "{text}");

        // 改行を含むメッセージは行ごとに出す(理由が枠幅で切れないようにするため)。
        let error = "ポート 4321 で待ち受けられません\n(Address already in use (os error 48))";
        let text = rendered(|frame| draw_host_setup(frame, 4, Some(error)));
        assert!(contains_text(&text, "対戦人数: 4人"), "{text}");
        for part in error.lines() {
            assert!(
                contains_text(&text, part),
                "失敗の理由が切れずに見えること: {part} / {text}"
            );
        }
    }

    #[test]
    fn join_input_shows_the_typed_address_and_an_error() {
        // 未入力でも入力欄とヒントが出る。
        let text = rendered(|frame| draw_join_input(frame, "", None));
        assert!(text.contains("JOIN"));
        assert!(contains_text(&text, "Backspace"));

        let error = "接続に失敗しました\n(Connection refused (os error 61))";
        let text = rendered(|frame| draw_join_input(frame, "192.168.1.10:4321", Some(error)));
        assert!(text.contains("192.168.1.10:4321"), "打った内容が見えること");
        for part in error.lines() {
            assert!(
                contains_text(&text, part),
                "失敗の理由が切れずに見えること: {part} / {text}"
            );
        }
    }

    #[test]
    fn menu_screens_render_on_a_small_terminal() {
        // 枠より狭い端末でも(切り詰められるだけで)panicしないこと。
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).expect("test terminal");
        terminal
            .draw(|frame| draw_mode_select(frame, 1))
            .expect("draw must not fail");
        terminal
            .draw(|frame| draw_host_setup(frame, 2, Some("エラー")))
            .expect("draw must not fail");
        terminal
            .draw(|frame| draw_join_input(frame, "127.0.0.1:4321", Some("エラー")))
            .expect("draw must not fail");
    }
}
