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

    // 表示スケール(+/-キーで変更)。ゲームロジックには影響しないUI専用の状態なので
    // GameStateには持たせず、ここで保持して render::draw にだけ渡す。
    let mut zoom = render::DEFAULT_ZOOM;

    let dt_secs = TICK_RATE.as_secs_f32();

    loop {
        let frame_start = Instant::now();

        let raw_action = input.poll_action();
        if matches!(raw_action, Action::Quit) {
            break;
        }

        // ズーム変更はUI操作であり、ゲーム進行(移動/設置)としては扱わない。
        let game_action = match raw_action {
            Action::ZoomIn => {
                zoom = (zoom + 1).min(render::MAX_ZOOM);
                Action::None
            }
            Action::ZoomOut => {
                zoom = zoom.saturating_sub(1).max(render::MIN_ZOOM);
                Action::None
            }
            other => other,
        };

        state.tick(dt_secs, game_action, &mut audio);

        terminal.draw(|frame| render::draw(frame, &state, zoom))?;

        let elapsed = frame_start.elapsed();
        if elapsed < TICK_RATE {
            std::thread::sleep(TICK_RATE - elapsed);
        }
    }

    Ok(())
}
