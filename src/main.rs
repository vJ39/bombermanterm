//! エントリポイント。
//!
//! crossterm raw mode / alternate screen の有効化・復帰は `ratatui::init` /
//! `ratatui::restore` (ratatui 0.30 の同梱ヘルパー、内部で crossterm を使う)に委譲する。
//! 固定tick(約33ms)のメインループで
//! `KeyboardInput::poll_action` → `GameState::tick` → `render::draw` の順に回し、
//! `Action::Quit` でループを抜けて端末状態を復元する。
//!
//! 起動モードは3つ(`clap` でパース):
//! - 引数なし … 従来どおりローカル1人プレイ+CPU敵([`run_local`])
//! - `host` … サーバー兼プレイヤー。ゲームロジックを回すのはこのモードだけ([`run_host`])
//! - `join` … クライアント。入力を送り、届いた状態を描画するだけ([`run_client`])
//!
//! ネットワーク対戦でも**メインループはこれまで通り同期**のまま。tokioは
//! [`net`] 側が起動する専用スレッドの中だけで動き、`std::sync::mpsc` で橋渡しする。

mod audio;
mod game;
mod input;
mod net;
mod render;
mod types;

use std::io;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};

use audio::{AudioPlayer, RodioPlayer};
use game::state::{GameState, MAX_PLAYERS, MIN_MULTIPLAYER_PLAYERS};
use input::{InputSource, KeyboardInput};
use net::client::ClientHandle;
use net::server::ServerHandle;
use types::{Action, Bgm};

/// 1tickの目標時間(約33ms ≒ 30fps相当)。
const TICK_RATE: Duration = Duration::from_millis(33);

#[derive(Parser)]
#[command(
    name = "bombermanterm",
    about = "ターミナルで遊ぶオリジナル爆弾アクション",
    version
)]
struct Cli {
    #[command(subcommand)]
    mode: Option<Mode>,
}

#[derive(Subcommand)]
enum Mode {
    /// ホスト(サーバー兼プレイヤー)として起動し、他プレイヤーの参加を待つ。
    Host {
        /// 待ち受けるポート番号。
        #[arg(short, long, default_value_t = net::DEFAULT_PORT)]
        port: u16,
        /// 対戦人数(ホスト自身を含む)。
        #[arg(short = 'n', long, default_value_t = 3,
              value_parser = clap::value_parser!(u8).range(MIN_MULTIPLAYER_PLAYERS as i64..=MAX_PLAYERS as i64))]
        players: u8,
    },
    /// クライアントとしてホストへ参加する。
    Join {
        /// 接続先。`host:port` 形式(例 `192.168.1.10:4321`)。
        addr: String,
    },
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // ネットワークの準備(bind / connect)はTUIを起動する前に済ませる。
    // 失敗した場合、代替画面へ切り替える前なのでエラーがそのまま端末に残る。
    match cli.mode {
        None => with_terminal(run_local),
        Some(Mode::Host { port, players }) => {
            let players = usize::from(players);
            let server = net::server::spawn(port, players - 1)?;
            println!(
                "{} で待ち受けます。{}人揃ったらホストの画面で SPACE を押してください。",
                server.local_addr(),
                players
            );
            with_terminal(|terminal| run_host(terminal, server, players))
        }
        Some(Mode::Join { addr }) => {
            let client = net::client::spawn(&addr)?;
            println!(
                "{addr} へ PLAYER {} として参加しました。",
                client.player_id + 1
            );
            with_terminal(|terminal| run_client(terminal, client, &addr))
        }
    }
}

/// `body` の実行中にpanicしても端末を元へ戻すためのガード。
///
/// 通常の `Err` 経路は呼び出し側で `ratatui::restore()` を直接呼べば済むが、
/// `body` 内でpanicするとその呼び出しまで到達せず、raw mode/代替画面に
/// 入ったままの端末が残ってユーザーの手元シェルが壊れて見える。Dropは
/// panicによる巻き戻し(unwind)でも実行されるため、ここに復元処理を置く。
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// 端末を代替画面へ切り替えて `body` を実行し、終了時(panicを含む)に必ず元へ戻す。
fn with_terminal<F>(body: F) -> io::Result<()>
where
    F: FnOnce(&mut ratatui::DefaultTerminal) -> io::Result<()>,
{
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;
    body(&mut terminal)
}

/// 1tickの残り時間だけ寝て、次のtickまでの間隔を揃える。
fn sleep_until_next_tick(frame_start: Instant) {
    let elapsed = frame_start.elapsed();
    if elapsed < TICK_RATE {
        std::thread::sleep(TICK_RATE - elapsed);
    }
}

/// ズーム操作(UI専用)を取り除き、ゲームへ渡すアクションだけを返す。
///
/// 表示スケールはゲーム進行(移動/設置)ではないので `GameState` へは渡さず、
/// 呼び出し側が持つ `zoom` を直接書き換える。
fn apply_zoom(action: Action, zoom: &mut usize) -> Action {
    match action {
        Action::ZoomIn => {
            *zoom = (*zoom + 1).min(render::MAX_ZOOM);
            Action::None
        }
        Action::ZoomOut => {
            *zoom = zoom.saturating_sub(1).max(render::MIN_ZOOM);
            Action::None
        }
        other => other,
    }
}

/// 起動時のオンボーディング画面を表示し、何らかの入力を待つ。
/// `Ok(true)` なら通常通り続行、`Ok(false)` ならユーザーがこの画面でQuitした。
fn show_intro(terminal: &mut ratatui::DefaultTerminal, input: &mut KeyboardInput) -> io::Result<bool> {
    loop {
        let frame_start = Instant::now();

        let action = input.poll_action();
        if matches!(action, Action::Quit) {
            return Ok(false);
        }
        if !matches!(action, Action::None) {
            return Ok(true);
        }

        terminal.draw(render::draw_intro)?;
        sleep_until_next_tick(frame_start);
    }
}

/// ローカル1人プレイ+CPU敵(引数なしで起動したときの従来の経路)。
fn run_local(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
    let mut input = KeyboardInput::new();
    if !show_intro(terminal, &mut input)? {
        return Ok(()); // イントロ画面でQuitされた。
    }

    let mut audio = RodioPlayer::new();
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

        let game_action = apply_zoom(raw_action, &mut zoom);

        state.tick(dt_secs, game_action, &mut audio);

        terminal.draw(|frame| render::draw(frame, &state, zoom))?;

        sleep_until_next_tick(frame_start);
    }

    Ok(())
}

/// ホスト(サーバー兼プレイヤー0)。
///
/// ゲームロジックを回すのはこのループだけで、クライアントへは結果だけを配る
/// (サーバー権威モデル)。1tickの流れ:
/// 1. 自分のキーボード入力を読む(プレイヤー0の入力)
/// 2. クライアントの入力([`ServerHandle::latest_client_inputs`])と結合する
/// 3. `GameState::tick_multi` で状態を進める
/// 4. 最新状態を全クライアントへ配り、自分の画面も描画する
fn run_host(
    terminal: &mut ratatui::DefaultTerminal,
    server: ServerHandle,
    num_players: usize,
) -> io::Result<()> {
    let mut audio = RodioPlayer::new();
    let mut input = KeyboardInput::new();
    let mut state = GameState::new_multiplayer(num_players);
    // 参加者が揃うまではロビー画面。揃ったらホストのSPACEで対戦が始まる。
    state.enter_lobby(num_players);

    audio.play_bgm(Bgm::Title);

    let mut zoom = render::DEFAULT_ZOOM;
    let dt_secs = TICK_RATE.as_secs_f32();

    loop {
        let frame_start = Instant::now();

        let raw_action = input.poll_action();
        if matches!(raw_action, Action::Quit) {
            break;
        }
        let own_action = apply_zoom(raw_action, &mut zoom);

        // 添字0(ホストの枠)は必ず空で届くので、自分の入力で埋める。
        // 何も届いていないtickは全員入力なしとして扱う(前tickの入力を
        // 使い回すと、キーを離しても動き続けてしまう)。
        let mut actions = server
            .latest_client_inputs()
            .unwrap_or_else(|| vec![Action::None; MAX_PLAYERS]);
        actions[0] = own_action;

        // 切断は「入力が来なくなる」だけではGameStateに伝わらないため、
        // 明示的にプレイヤーを退場させる(でないと最後の相手が切断しても
        // 対戦の決着判定が働かず試合が終わらない)。
        for player_id in server.take_disconnected() {
            state.retire_player(player_id);
        }

        state.set_lobby_connected(server.player_count());
        state.tick_multi(dt_secs, &actions, &mut audio);

        // 送信できなくてもホストのプレイは続けられるので、失敗は無視する。
        let _ = server.send_snapshot(&state);

        terminal.draw(|frame| render::draw_with_perspective(frame, &state, zoom, Some(0)))?;

        sleep_until_next_tick(frame_start);
    }

    Ok(())
}

/// クライアント。ロジックは一切実行せず、入力の送信と受信状態の描画だけを行う。
///
/// 音もホスト側で鳴るのでここでは鳴らさない(`RodioPlayer` も作らない)。
fn run_client(
    terminal: &mut ratatui::DefaultTerminal,
    client: ClientHandle,
    addr: &str,
) -> io::Result<()> {
    let mut input = KeyboardInput::new();
    let mut zoom = render::DEFAULT_ZOOM;
    // 最初のスナップショットが届くまでは描くものが無い。
    let mut state: Option<Box<GameState>> = None;

    loop {
        let frame_start = Instant::now();

        let raw_action = input.poll_action();
        if matches!(raw_action, Action::Quit) {
            break;
        }
        let action = apply_zoom(raw_action, &mut zoom);

        // 入力が無いtickは送らない(サーバーは届いた入力だけを消費する)。
        if !matches!(action, Action::None) && client.send(action).is_err() {
            break;
        }

        match client.latest_snapshot() {
            Ok(Some(latest)) => state = Some(latest),
            Ok(None) => {}
            // ホストが終了した/回線が切れた。端末を復帰させて終わる。
            Err(net::client::Disconnected) => break,
        }

        terminal.draw(|frame| match state.as_deref() {
            Some(state) => {
                render::draw_with_perspective(frame, state, zoom, Some(client.player_id))
            }
            None => render::draw_connecting(frame, addr),
        })?;

        sleep_until_next_tick(frame_start);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_means_local_single_player() {
        let cli = Cli::try_parse_from(["bombermanterm"]).expect("parse");
        assert!(cli.mode.is_none());
    }

    #[test]
    fn host_defaults_to_the_shared_port_and_three_players() {
        let cli = Cli::try_parse_from(["bombermanterm", "host"]).expect("parse");
        let Some(Mode::Host { port, players }) = cli.mode else {
            panic!("expected the host mode");
        };
        assert_eq!(port, net::DEFAULT_PORT);
        assert_eq!(players, 3);
    }

    #[test]
    fn host_accepts_an_explicit_port_and_player_count() {
        let cli =
            Cli::try_parse_from(["bombermanterm", "host", "--port", "5000", "--players", "4"])
                .expect("parse");
        let Some(Mode::Host { port, players }) = cli.mode else {
            panic!("expected the host mode");
        };
        assert_eq!(port, 5000);
        assert_eq!(players, 4);
    }

    #[test]
    fn host_rejects_player_counts_outside_the_supported_range() {
        for players in ["1", "5", "0"] {
            assert!(
                Cli::try_parse_from(["bombermanterm", "host", "--players", players]).is_err(),
                "{players}人は対戦人数として受け付けない"
            );
        }
    }

    #[test]
    fn join_takes_a_host_and_port_address() {
        let cli = Cli::try_parse_from(["bombermanterm", "join", "127.0.0.1:4321"]).expect("parse");
        let Some(Mode::Join { addr }) = cli.mode else {
            panic!("expected the join mode");
        };
        assert_eq!(addr, "127.0.0.1:4321");

        // 接続先の指定は必須。
        assert!(Cli::try_parse_from(["bombermanterm", "join"]).is_err());
    }

    #[test]
    fn apply_zoom_consumes_zoom_keys_and_passes_game_actions_through() {
        let mut zoom = render::DEFAULT_ZOOM;

        assert_eq!(apply_zoom(Action::ZoomIn, &mut zoom), Action::None);
        assert_eq!(zoom, render::DEFAULT_ZOOM + 1);

        assert_eq!(apply_zoom(Action::ZoomOut, &mut zoom), Action::None);
        assert_eq!(zoom, render::DEFAULT_ZOOM);

        // 下限・上限を超えない。
        for _ in 0..10 {
            apply_zoom(Action::ZoomOut, &mut zoom);
        }
        assert_eq!(zoom, render::MIN_ZOOM);
        for _ in 0..10 {
            apply_zoom(Action::ZoomIn, &mut zoom);
        }
        assert_eq!(zoom, render::MAX_ZOOM);

        // ゲーム操作はそのまま通す。
        assert_eq!(apply_zoom(Action::PlaceBomb, &mut zoom), Action::PlaceBomb);
    }
}
