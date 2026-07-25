//! 起動時のモード選択メニュー(引数なしで起動したときの経路)。
//!
//! `GameState` にも `Action` にも依存しない独立した画面として実装する。以前
//! `Screen` へ画面を足そうとして既存のゲームロジックのテストを巻き込んだので、
//! メニューの状態は [`MenuState`] としてここに閉じ、`GameState` の外で完結させる。
//!
//! 遷移は副作用の無い [`step`] に集約し、イベントループ([`run_from`])は
//! 「キーを読む → `step` → 描く」だけを行う。これで遷移仕様はユニットテストで
//! 検証できる(`apply_zoom` と同じ切り分け)。
//!
//! テキスト入力(接続先アドレス)を扱うため、キー入力は
//! [`crate::input::KeyboardInput`] を通さず crossterm を直接読む。

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;

use crate::game::state::{MAX_PLAYERS, MIN_MULTIPLAYER_PLAYERS};
use crate::render;

/// モード選択の項目数(ローカル/ホスト/参加)。表示ラベルは
/// `render::menu::MODE_LABELS` 側に持ち、件数の一致はテストで担保する。
pub(crate) const MODE_COUNT: usize = 3;

/// モード選択の添字。
const MODE_LOCAL: usize = 0;
const MODE_HOST: usize = 1;
const MODE_JOIN: usize = 2;

/// ホスト設定画面を開いたときの初期人数。CLIの `--players` の既定値と揃える。
const DEFAULT_HOST_PLAYERS: usize = 3;

/// メニューが今どの画面に居るか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuState {
    /// モード選択。`selected` は `render::menu::MODE_LABELS` の添字。
    ModeSelect { selected: usize },
    /// ホストの対戦人数設定。`error` は直前の待ち受け失敗の理由。
    HostSetup {
        players: usize,
        error: Option<String>,
    },
    /// 参加先アドレスの入力。`error` は直前の接続失敗の理由。
    JoinInput { addr: String, error: Option<String> },
}

/// メニューで選ばれた起動内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchChoice {
    Local,
    Host { players: usize },
    Join { addr: String },
}

/// 1キー分の遷移結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    Continue(MenuState),
    Launch(LaunchChoice),
    Quit,
}

/// メニューを抜けた理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuOutcome {
    Quit,
    Launch(LaunchChoice),
}

/// モード選択から始めるメニュー。
pub fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<MenuOutcome> {
    run_from(
        terminal,
        MenuState::ModeSelect {
            selected: MODE_LOCAL,
        },
    )
}

/// `initial` の画面から始めるメニュー。
///
/// 接続や待ち受けに失敗したあと、入力内容とエラーを保ったまま同じ画面へ
/// 戻すために使う。
pub fn run_from(
    terminal: &mut ratatui::DefaultTerminal,
    initial: MenuState,
) -> io::Result<MenuOutcome> {
    let mut state = initial;

    loop {
        let frame_start = Instant::now();

        // 溜まっているキーはこのtickで全部消費する(1tickに1キーだと、
        // アドレスを速く打った/貼り付けたときに反映が遅れて見える)。
        while event::poll(Duration::from_millis(0))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            // Press/Release の両方を送る環境があるため Release は捨てる
            // (`KeyboardInput` と同じ理由。拾うと1打鍵で2回進む)。
            if key.kind == KeyEventKind::Release {
                continue;
            }

            match step(state, key) {
                StepOutcome::Continue(next) => state = next,
                StepOutcome::Launch(choice) => return Ok(MenuOutcome::Launch(choice)),
                StepOutcome::Quit => return Ok(MenuOutcome::Quit),
            }
        }

        terminal.draw(|frame| draw(frame, &state))?;

        crate::sleep_until_next_tick(frame_start);
    }
}

/// 現在の画面を描画する。
fn draw(frame: &mut Frame, state: &MenuState) {
    match state {
        MenuState::ModeSelect { selected } => render::draw_mode_select(frame, *selected),
        MenuState::HostSetup { players, error } => {
            render::draw_host_setup(frame, *players, error.as_deref())
        }
        MenuState::JoinInput { addr, error } => {
            render::draw_join_input(frame, addr, error.as_deref())
        }
    }
}

/// キー1つ分の遷移。副作用は持たないので、そのままユニットテストできる。
fn step(state: MenuState, key: KeyEvent) -> StepOutcome {
    match state {
        MenuState::ModeSelect { selected } => step_mode_select(selected, key),
        MenuState::HostSetup { players, error } => step_host_setup(players, error, key),
        MenuState::JoinInput { addr, error } => step_join_input(addr, error, key),
    }
}

fn step_mode_select(selected: usize, key: KeyEvent) -> StepOutcome {
    let selected = selected.min(MODE_COUNT - 1);

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => StepOutcome::Continue(MenuState::ModeSelect {
            selected: (selected + MODE_COUNT - 1) % MODE_COUNT,
        }),
        KeyCode::Down | KeyCode::Char('j') => StepOutcome::Continue(MenuState::ModeSelect {
            selected: (selected + 1) % MODE_COUNT,
        }),
        KeyCode::Enter => match selected {
            MODE_HOST => StepOutcome::Continue(MenuState::HostSetup {
                players: DEFAULT_HOST_PLAYERS,
                error: None,
            }),
            MODE_JOIN => StepOutcome::Continue(MenuState::JoinInput {
                addr: String::new(),
                error: None,
            }),
            // MODE_LOCAL。設定項目が無いのでそのまま起動する。
            _ => StepOutcome::Launch(LaunchChoice::Local),
        },
        KeyCode::Esc | KeyCode::Char('q') => StepOutcome::Quit,
        _ => StepOutcome::Continue(MenuState::ModeSelect { selected }),
    }
}

fn step_host_setup(players: usize, error: Option<String>, key: KeyEvent) -> StepOutcome {
    let players = players.clamp(MIN_MULTIPLAYER_PLAYERS, MAX_PLAYERS);

    match key.code {
        KeyCode::Left | KeyCode::Char('h') => StepOutcome::Continue(MenuState::HostSetup {
            players: (players - 1).max(MIN_MULTIPLAYER_PLAYERS),
            error,
        }),
        KeyCode::Right | KeyCode::Char('l') => StepOutcome::Continue(MenuState::HostSetup {
            players: (players + 1).min(MAX_PLAYERS),
            error,
        }),
        KeyCode::Enter => StepOutcome::Launch(LaunchChoice::Host { players }),
        KeyCode::Esc => StepOutcome::Continue(MenuState::ModeSelect {
            selected: MODE_HOST,
        }),
        _ => StepOutcome::Continue(MenuState::HostSetup { players, error }),
    }
}

fn step_join_input(mut addr: String, error: Option<String>, key: KeyEvent) -> StepOutcome {
    match key.code {
        // Ctrl/Alt 付きの文字はアドレスに混ぜない(Ctrl+C で 'c' が入るのを防ぐ)。
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            addr.push(c);
            // 入力が変わった時点で前回の失敗理由は当てはまらないので消す。
            StepOutcome::Continue(MenuState::JoinInput { addr, error: None })
        }
        KeyCode::Backspace => {
            addr.pop();
            StepOutcome::Continue(MenuState::JoinInput { addr, error: None })
        }
        // 空のまま接続しようとしても何も起きない(接続先が決まっていない)。
        KeyCode::Enter if !addr.trim().is_empty() => {
            StepOutcome::Launch(LaunchChoice::Join { addr })
        }
        KeyCode::Esc => StepOutcome::Continue(MenuState::ModeSelect {
            selected: MODE_JOIN,
        }),
        _ => StepOutcome::Continue(MenuState::JoinInput { addr, error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mode_select(selected: usize) -> MenuState {
        MenuState::ModeSelect { selected }
    }

    fn host_setup(players: usize) -> MenuState {
        MenuState::HostSetup {
            players,
            error: None,
        }
    }

    fn join_input(addr: &str) -> MenuState {
        MenuState::JoinInput {
            addr: addr.to_string(),
            error: None,
        }
    }

    /// `Continue` を前提に次の状態を取り出す(遷移の連結を書きやすくするため)。
    fn continued(outcome: StepOutcome) -> MenuState {
        match outcome {
            StepOutcome::Continue(state) => state,
            other => panic!("Continue を期待したが {other:?}"),
        }
    }

    #[test]
    fn mode_select_moves_up_and_down_and_wraps_around() {
        for code in [KeyCode::Down, KeyCode::Char('j')] {
            assert_eq!(
                continued(step(mode_select(0), key(code))),
                mode_select(1),
                "{code:?} で下へ"
            );
            // 末尾から先頭へ回り込む。
            assert_eq!(continued(step(mode_select(2), key(code))), mode_select(0));
        }

        for code in [KeyCode::Up, KeyCode::Char('k')] {
            assert_eq!(
                continued(step(mode_select(1), key(code))),
                mode_select(0),
                "{code:?} で上へ"
            );
            // 先頭から末尾へ回り込む。
            assert_eq!(continued(step(mode_select(0), key(code))), mode_select(2));
        }
    }

    #[test]
    fn mode_select_enter_starts_local_play_or_opens_the_setup_screen() {
        assert_eq!(
            step(mode_select(MODE_LOCAL), key(KeyCode::Enter)),
            StepOutcome::Launch(LaunchChoice::Local)
        );
        assert_eq!(
            continued(step(mode_select(MODE_HOST), key(KeyCode::Enter))),
            host_setup(DEFAULT_HOST_PLAYERS),
            "ホストの既定人数はCLIの --players と同じ"
        );
        assert_eq!(
            continued(step(mode_select(MODE_JOIN), key(KeyCode::Enter))),
            join_input(""),
            "参加は空のアドレス入力から始まる"
        );
    }

    #[test]
    fn mode_select_quits_on_esc_or_q() {
        for code in [KeyCode::Esc, KeyCode::Char('q')] {
            assert_eq!(step(mode_select(0), key(code)), StepOutcome::Quit);
        }
    }

    #[test]
    fn mode_select_ignores_unrelated_keys() {
        // ゲーム中の設置キー(Space)で誤って起動しないこと(イントロを閉じた
        // 打鍵が残っていても暴発しない)。
        for code in [KeyCode::Char(' '), KeyCode::Char('z'), KeyCode::Tab] {
            assert_eq!(continued(step(mode_select(1), key(code))), mode_select(1));
        }
    }

    #[test]
    fn host_setup_changes_the_player_count_within_the_supported_range() {
        for code in [KeyCode::Right, KeyCode::Char('l')] {
            assert_eq!(continued(step(host_setup(2), key(code))), host_setup(3));
            // 上限で止まる。
            assert_eq!(
                continued(step(host_setup(MAX_PLAYERS), key(code))),
                host_setup(MAX_PLAYERS)
            );
        }

        for code in [KeyCode::Left, KeyCode::Char('h')] {
            assert_eq!(continued(step(host_setup(3), key(code))), host_setup(2));
            // 下限で止まる。
            assert_eq!(
                continued(step(host_setup(MIN_MULTIPLAYER_PLAYERS), key(code))),
                host_setup(MIN_MULTIPLAYER_PLAYERS)
            );
        }
    }

    #[test]
    fn host_setup_keeps_the_error_until_the_player_count_matters() {
        // 待ち受け失敗のメッセージは人数を変えても残す(原因が変わらないため)。
        let state = MenuState::HostSetup {
            players: 3,
            error: Some("ポートが使用中です".to_string()),
        };
        assert_eq!(
            continued(step(state, key(KeyCode::Right))),
            MenuState::HostSetup {
                players: 4,
                error: Some("ポートが使用中です".to_string())
            }
        );
    }

    #[test]
    fn host_setup_enter_launches_and_esc_goes_back_to_mode_select() {
        assert_eq!(
            step(host_setup(4), key(KeyCode::Enter)),
            StepOutcome::Launch(LaunchChoice::Host { players: 4 })
        );
        assert_eq!(
            continued(step(host_setup(3), key(KeyCode::Esc))),
            mode_select(MODE_HOST),
            "戻り先はホストの項目"
        );
    }

    #[test]
    fn join_input_types_and_deletes_characters() {
        let mut state = join_input("");
        for c in "127.0.0.1:4321".chars() {
            state = continued(step(state, key(KeyCode::Char(c))));
        }
        assert_eq!(state, join_input("127.0.0.1:4321"));

        state = continued(step(state, key(KeyCode::Backspace)));
        assert_eq!(state, join_input("127.0.0.1:432"));

        // 空になってもさらに削除してよい(panicしない)。
        let mut empty = join_input("");
        empty = continued(step(empty, key(KeyCode::Backspace)));
        assert_eq!(empty, join_input(""));
    }

    #[test]
    fn join_input_editing_clears_the_previous_error() {
        let state = MenuState::JoinInput {
            addr: "127.0.0.1:4321".to_string(),
            error: Some("接続に失敗しました".to_string()),
        };
        // アドレスを変え始めたら前回の失敗理由は当てはまらない。
        assert_eq!(
            continued(step(state, key(KeyCode::Backspace))),
            join_input("127.0.0.1:432")
        );
    }

    #[test]
    fn join_input_does_not_type_control_combinations() {
        let state = join_input("127.0.0.1");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(continued(step(state, ctrl_c)), join_input("127.0.0.1"));
    }

    #[test]
    fn join_input_needs_an_address_before_connecting() {
        // 空(および空白だけ)では接続しない。
        for addr in ["", " "] {
            assert_eq!(
                continued(step(join_input(addr), key(KeyCode::Enter))),
                join_input(addr),
                "{addr:?} では接続しない"
            );
        }

        assert_eq!(
            step(join_input("127.0.0.1:4321"), key(KeyCode::Enter)),
            StepOutcome::Launch(LaunchChoice::Join {
                addr: "127.0.0.1:4321".to_string()
            })
        );
    }

    #[test]
    fn join_input_esc_goes_back_to_mode_select() {
        assert_eq!(
            continued(step(join_input("127.0.0.1"), key(KeyCode::Esc))),
            mode_select(MODE_JOIN),
            "戻り先は参加の項目"
        );
    }
}
