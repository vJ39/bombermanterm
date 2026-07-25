//! キー入力→アクション変換。
//!
//! crossterm event::poll/read を使い、矢印キー+hjkl+Space+Esc/q をノンブロッキングに
//! Action へ変換する。将来ネットワーク対戦時に入力ソースを差し替えられるようtrait化する。

use crate::types::{Action, Direction};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::time::Duration;

pub trait InputSource {
    fn poll_action(&mut self) -> Action;
}

pub struct KeyboardInput;

impl KeyboardInput {
    pub fn new() -> Self {
        KeyboardInput
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
                    _ => Action::None,
                }
            }
            _ => Action::None,
        }
    }
}
