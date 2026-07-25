//! エントリポイント。
//!
//! crossterm raw mode / alternate screen の有効化・復帰は `ratatui::init` /
//! `ratatui::restore` (ratatui 0.30 の同梱ヘルパー、内部で crossterm を使う)に委譲する。
//! 固定tick(約33ms)のメインループで
//! `KeyboardInput::poll_action` → `GameState::tick` → `render::draw` の順に回し、
//! `Action::Quit` でループを抜けて端末状態を復元する。

mod audio;
mod game;
mod input;
mod render;
mod types;

use std::io;
use std::time::{Duration, Instant};

use audio::{AudioPlayer, RodioPlayer};
use game::state::GameState;
use input::{InputSource, KeyboardInput};
use types::{Action, Bgm};

/// 1tickの目標時間(約33ms ≒ 30fps相当)。
const TICK_RATE: Duration = Duration::from_millis(33);

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
    let mut audio = RodioPlayer::new();
    let mut input = KeyboardInput::new();
    let mut state = GameState::new();

    // GameState::new() は Screen::Title で始まるが、audio を持たないため
    // タイトルBGMの再生はここで明示的にキックする。以降の画面遷移時のBGM切替は
    // GameState::tick 内で行われる。
    audio.play_bgm(Bgm::Title);

    let dt_secs = TICK_RATE.as_secs_f32();

    loop {
        let frame_start = Instant::now();

        let action = input.poll_action();
        if matches!(action, Action::Quit) {
            break;
        }

        state.tick(dt_secs, action, &mut audio);

        terminal.draw(|frame| render::draw(frame, &state))?;

        let elapsed = frame_start.elapsed();
        if elapsed < TICK_RATE {
            std::thread::sleep(TICK_RATE - elapsed);
        }
    }

    Ok(())
}
