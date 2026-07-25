//! ネットワーク対戦(サーバー権威モデル)。
//!
//! 1人がホスト(サーバー)を兼ね、残り最大3人がクライアントとして接続する
//! リスンサーバー方式。ホストのメインループだけが [`crate::game::state::GameState`]
//! を更新し、クライアントは入力を送って結果を受け取るだけの薄いクライアントになる。
//! 全員で同じロジックを走らせるロックステップ方式ではないので、乱数や浮動小数点の
//! 同期ズレを気にする必要がない。
//!
//! tokioの非同期ランタイムは [`server::spawn`] / [`client::spawn`] が起動する
//! 専用スレッドの中だけで動き、メインスレッドの同期ループ(crossterm + ratatui)は
//! これまで通りのまま。両者の橋渡しは `std::sync::mpsc` で行う。

pub mod client;
pub mod protocol;
pub mod server;

use std::time::Duration;

/// `--port` を省略したときのポート番号。
pub const DEFAULT_PORT: u16 = 4321;

/// サーバーがクライアントの入力をまとめてメインスレッドへ渡す間隔。
///
/// メインループのtick(約33ms ≒ 30fps)と揃えてある。溜まったセットは
/// [`server::ServerHandle::latest_client_inputs`] が畳んで返すので、
/// 多少ずれても入力が失われることはない。
pub const INPUT_COLLECT_INTERVAL: Duration = Duration::from_millis(33);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioPlayer;
    use crate::game::state::{GameState, MAX_PLAYERS};
    use crate::types::{Action, Bgm, Direction, Screen, SoundEffect};
    use std::time::Instant;

    /// 何も鳴らさない `AudioPlayer`。ホストのtickを回すために必要なだけの実装。
    struct SilentAudio;

    impl AudioPlayer for SilentAudio {
        fn play_se(&mut self, _se: SoundEffect) {}
        fn play_bgm(&mut self, _bgm: Bgm) {}
        fn stop_bgm(&mut self) {}
    }

    /// `condition` が真になるまで最大 `limit` 待つ。真になったら true。
    ///
    /// 実際のTCP越しのやり取りを検証するテスト向けの補助。固定の sleep で
    /// 決め打ちすると遅いマシンで不安定になるため、条件が満たされ次第進める。
    fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        condition()
    }

    /// 待ち時間の上限。CI等の遅い環境でも落ちないよう余裕を持たせる。
    const TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn host_and_clients_exchange_inputs_and_snapshots_over_loopback() {
        let server = server::spawn(0, MAX_PLAYERS - 1).expect("bind an ephemeral port");
        let addr = server.local_addr().to_string();

        // 1人目のクライアント。ホストが0番なので1番が割り当たる。
        let first = client::spawn(&addr).expect("first client connects");
        assert_eq!(first.player_id, 1);

        // 2人目は次の番号。ロビー表示用の人数もホストを含めて増える。
        let second = client::spawn(&addr).expect("second client connects");
        assert_eq!(second.player_id, 2);
        assert!(
            wait_until(TIMEOUT, || server.player_count() == 3),
            "ホスト+クライアント2人で3人と数える(実際は{})",
            server.player_count()
        );

        // クライアントの入力がホストのメインループ側へ届くこと。
        first
            .send(Action::Move(Direction::Left))
            .expect("send input");
        second.send(Action::PlaceBomb).expect("send input");

        let mut received = vec![Action::None; MAX_PLAYERS];
        let got_both = wait_until(TIMEOUT, || {
            if let Some(inputs) = server.latest_client_inputs() {
                for (slot, action) in received.iter_mut().zip(inputs) {
                    if !matches!(action, Action::None) {
                        *slot = action;
                    }
                }
            }
            !matches!(received[1], Action::None) && !matches!(received[2], Action::None)
        });
        assert!(got_both, "両方の入力が届くこと: {received:?}");
        assert_eq!(received[0], Action::None, "添字0はホストの予約枠");
        assert_eq!(received[1], Action::Move(Direction::Left));
        assert_eq!(received[2], Action::PlaceBomb);

        // ホストが配ったスナップショットが両クライアントへ届くこと。
        let mut state = GameState::new_multiplayer(3);
        state.screen = Screen::Playing;
        state.players[1].score = 777;
        server.send_snapshot(&state).expect("broadcast a snapshot");

        for (label, client) in [("first", &first), ("second", &second)] {
            let mut snapshot = None;
            let arrived = wait_until(TIMEOUT, || {
                match client.latest_snapshot() {
                    Ok(Some(state)) => {
                        snapshot = Some(state);
                        true
                    }
                    Ok(None) => false,
                    // 切断されたらこれ以上待っても届かない。
                    Err(client::Disconnected) => true,
                }
            });
            assert!(arrived, "{label} client must receive the snapshot");

            let snapshot = snapshot.unwrap_or_else(|| panic!("{label} client got disconnected"));
            assert_eq!(snapshot.screen, Screen::Playing);
            assert_eq!(snapshot.players.len(), 3);
            assert_eq!(snapshot.players[1].score, 777);
            assert_eq!(snapshot.map.width, state.map.width);
        }

        // 切断すると空き番号が戻り、次のクライアントがその番号を引き継ぐ。
        drop(first);
        assert!(
            wait_until(TIMEOUT, || server.connected_clients() == 1),
            "切断が反映されること(実際は{})",
            server.connected_clients()
        );

        let third = client::spawn(&addr).expect("third client connects");
        assert_eq!(third.player_id, 1, "空いた番号を再利用する");
    }

    #[test]
    fn a_client_input_reaches_the_hosts_game_logic() {
        // `main.rs` のホストループと同じ順序(入力の取り込み → tick_multi)を
        // テスト内で再現し、クライアントの入力がゲームの結果まで届くことを確認する。
        let server = server::spawn(0, 1).expect("bind an ephemeral port");
        let client = client::spawn(&server.local_addr().to_string()).expect("client connects");
        assert_eq!(client.player_id, 1);

        let mut audio = SilentAudio;
        let mut state = GameState::new_multiplayer(2);
        let dt = INPUT_COLLECT_INTERVAL.as_secs_f32();

        // ホストのSPACEで対戦開始。
        state.tick_multi(dt, &[Action::PlaceBomb], &mut audio);
        assert_eq!(state.screen, Screen::Playing);
        assert!(state.bombs.is_empty());

        // ホストのループを1tickだけ進める(自分の入力は無し)。
        let mut step_host = |state: &mut GameState| {
            let mut actions = server
                .latest_client_inputs()
                .unwrap_or_else(|| vec![Action::None; MAX_PLAYERS]);
            actions[0] = Action::None;
            state.tick_multi(dt, &actions, &mut audio);
        };

        // ボム設置は初期位置に依存しないので、経路の検証に使いやすい。
        client.send(Action::PlaceBomb).expect("send input");
        let placed = wait_until(TIMEOUT, || {
            step_host(&mut state);
            state.bombs.iter().any(|bomb| bomb.owner == 1)
        });
        assert!(
            placed,
            "クライアントの入力がホストのゲームロジックへ届くこと"
        );
        assert!(
            !state.bombs.iter().any(|bomb| bomb.owner == 0),
            "ホスト自身は入力していないのでホストのボムは置かれない"
        );

        // 移動も同じ経路で効くこと。壁で塞がれた方向を選ばないよう、
        // 進入可能な隣接マスがある方向を盤面から選ぶ。
        let start = state.players[1].pos;
        let host_pos = state.players[0].pos;
        let direction = [
            Direction::Up,
            Direction::Down,
            Direction::Left,
            Direction::Right,
        ]
        .into_iter()
        .find(|&dir| {
            let target = match dir {
                Direction::Up => (start.0 - 1, start.1),
                Direction::Down => (start.0 + 1, start.1),
                Direction::Left => (start.0, start.1 - 1),
                Direction::Right => (start.0, start.1 + 1),
            };
            state.map.is_walkable(target)
        });

        if let Some(direction) = direction {
            client.send(Action::Move(direction)).expect("send input");
            let moved = wait_until(TIMEOUT, || {
                step_host(&mut state);
                state.players[1].pos != start
            });
            assert!(moved, "クライアントの移動入力が反映されること");
            assert_eq!(
                state.players[0].pos, host_pos,
                "ホストのプレイヤーは動いていないこと"
            );
        }
    }

    #[test]
    fn a_client_disconnect_reaches_the_hosts_game_logic_and_retires_them() {
        // `main.rs::run_host` と同じ順序(take_disconnected → retire_player →
        // tick_multi)をテスト内で再現し、実際のTCP切断がGameStateまで
        // 届いて対戦の決着判定に反映されることを確認する。
        let server = server::spawn(0, 2).expect("bind an ephemeral port");
        let addr = server.local_addr().to_string();

        let first = client::spawn(&addr).expect("first client connects");
        assert_eq!(first.player_id, 1);
        let second = client::spawn(&addr).expect("second client connects");
        assert_eq!(second.player_id, 2);
        assert!(wait_until(TIMEOUT, || server.connected_clients() == 2));

        let mut audio = SilentAudio;
        let mut state = GameState::new_multiplayer(3);
        let dt = INPUT_COLLECT_INTERVAL.as_secs_f32();
        state.tick_multi(dt, &[Action::PlaceBomb, Action::None, Action::None], &mut audio);
        assert_eq!(state.screen, Screen::Playing);

        // 2人のクライアントを両方切断する(実際の対戦なら「相手が全員切断」に相当)。
        drop(first);
        drop(second);

        let retired = wait_until(TIMEOUT, || {
            for player_id in server.take_disconnected() {
                state.retire_player(player_id);
            }
            !state.players[1].alive && !state.players[2].alive
        });
        assert!(
            retired,
            "切断がGameStateへ届いてプレイヤーが退場すること: {:?}",
            state.players.iter().map(|p| p.alive).collect::<Vec<_>>()
        );

        // 次のtickで決着判定が走り、切断していないホスト(プレイヤー0)の勝ちになる。
        state.tick_multi(dt, &[Action::None; 3], &mut audio);
        assert_eq!(
            state.screen,
            Screen::MatchResult(Some(0)),
            "生き残ったホストが勝者として確定すること"
        );
    }

    #[test]
    fn server_rejects_connections_beyond_the_configured_player_count() {
        // ホスト+1人の2人対戦なら、受け入れるクライアントは1人だけ。
        let server = server::spawn(0, 1).expect("bind an ephemeral port");
        let addr = server.local_addr().to_string();

        let accepted = client::spawn(&addr).expect("the first client is accepted");
        assert_eq!(accepted.player_id, 1);
        assert!(wait_until(TIMEOUT, || server.connected_clients() == 1));

        // 2人目は枠が無いので、接続してもハンドシェイクが成立しない。
        // (サーバーは Welcome を送らずに接続を閉じる)
        let rejected = client::spawn(&addr);
        assert!(
            rejected.is_err(),
            "満員のサーバーへは参加できない: {:?}",
            rejected.err()
        );
        assert_eq!(
            server.connected_clients(),
            1,
            "受け入れ済みの1人は影響を受けない"
        );
    }
}
