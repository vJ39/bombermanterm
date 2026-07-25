//! キー入力→アクション変換。
//!
//! crossterm event::poll/read を使い、矢印キー+hjkl+Space+Esc/q をノンブロッキングに
//! Action へ変換する。将来ネットワーク対戦時に入力ソースを差し替えられるようtrait化する。

use crate::types::{Action, Direction};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::time::Duration;

/// 強制無敵モードON/OFFトグルの隠しコマンド(`god`と連続入力する)。
/// 移動キー(h/j/k/l)や設置(space)・終了(q/esc)と文字が被らないため、
/// 通常操作の途中で誤って発火することはない。
const GOD_MODE_CODE: &[char] = &['g', 'o', 'd'];

pub trait InputSource {
    fn poll_action(&mut self) -> Action;
}

pub struct KeyboardInput {
    /// 直近に入力された文字を `GOD_MODE_CODE` の長さ分だけ保持するバッファ。
    /// 隠しコマンドと一致したら消費して `Action::ToggleGodMode` を発火する。
    code_buffer: Vec<char>,
}

impl KeyboardInput {
    pub fn new() -> Self {
        KeyboardInput {
            code_buffer: Vec::with_capacity(GOD_MODE_CODE.len()),
        }
    }

    /// 通常キーとして解釈されなかった文字入力を隠しコマンドバッファへ積む。
    /// 一致したらバッファをクリアして true を返す。
    fn feed_code_buffer(&mut self, c: char) -> bool {
        self.code_buffer.push(c);
        if self.code_buffer.len() > GOD_MODE_CODE.len() {
            self.code_buffer.remove(0);
        }
        if self.code_buffer == GOD_MODE_CODE {
            self.code_buffer.clear();
            true
        } else {
            false
        }
    }
}

impl Default for KeyboardInput {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSource for KeyboardInput {
    fn poll_action(&mut self) -> Action {
        // ノンブロッキング: イベントが無ければ即座に false を返す。
        match event::poll(Duration::from_millis(0)) {
            Ok(true) => {}
            _ => return Action::None,
        }

        match event::read() {
            Ok(Event::Key(key_event)) => {
                // 一部プラットフォームは Press/Release/Repeat を送るため、
                // Press(または Kind情報が無い環境向けに Repeat も)のみ拾い、
                // Release で二重発火しないようにする。
                if key_event.kind == KeyEventKind::Release {
                    return Action::None;
                }

                match key_event.code {
                    KeyCode::Up | KeyCode::Char('k') => Action::Move(Direction::Up),
                    KeyCode::Down | KeyCode::Char('j') => Action::Move(Direction::Down),
                    KeyCode::Left | KeyCode::Char('h') => Action::Move(Direction::Left),
                    KeyCode::Right | KeyCode::Char('l') => Action::Move(Direction::Right),
                    KeyCode::Char(' ') => Action::PlaceBomb,
                    KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
                    KeyCode::Char(c) if self.feed_code_buffer(c) => Action::ToggleGodMode,
                    _ => Action::None,
                }
            }
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_code_buffer_recognizes_the_full_sequence() {
        let mut input = KeyboardInput::new();
        assert!(!input.feed_code_buffer('g'));
        assert!(!input.feed_code_buffer('o'));
        assert!(input.feed_code_buffer('d'));
    }

    #[test]
    fn feed_code_buffer_ignores_unrelated_keys_and_resets_after_match() {
        let mut input = KeyboardInput::new();
        assert!(!input.feed_code_buffer('x'));
        assert!(!input.feed_code_buffer('g'));
        assert!(!input.feed_code_buffer('o'));
        assert!(input.feed_code_buffer('d'));

        // 一致後はバッファがクリアされ、直後にもう一度同じ列を入れれば再度検出できる。
        assert!(!input.feed_code_buffer('g'));
        assert!(!input.feed_code_buffer('o'));
        assert!(input.feed_code_buffer('d'));
    }

    #[test]
    fn feed_code_buffer_keeps_only_a_trailing_window() {
        let mut input = KeyboardInput::new();
        // ノイズの後に完全な列が続けば検出できる(先頭のノイズは押し出される)。
        for c in ['z', 'z', 'z', 'g', 'o'] {
            assert!(!input.feed_code_buffer(c));
        }
        assert!(input.feed_code_buffer('d'));
    }
}
