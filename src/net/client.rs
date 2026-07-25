//! ネットワーク対戦のクライアント側。
//!
//! クライアントはゲームロジックを一切実行しない薄いクライアント。
//! 自分の入力をサーバーへ送り、サーバーから届いた [`GameState`] をそのまま
//! 描画するだけ(`tick`/`tick_multi` は呼ばない)。乱数や浮動小数点の
//! 計算結果がホストとズレる余地が無いのがこの方式の利点。
//!
//! サーバー側と同じく、tokioはこのモジュールが起動する専用スレッドの中だけで動き、
//! メインスレッドの同期ループとは `std::sync::mpsc` で橋渡しする。

use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::runtime::Runtime;

use crate::game::state::GameState;
use crate::net::protocol::{self, ClientMessage, ServerMessage};
use crate::types::Action;

/// 接続と `Welcome` 受信をここまで待つ。
///
/// 到達できないアドレスを指定した場合にOSの既定タイムアウト(数十秒)まで
/// TUIが起動せず固まるのを避けるための上限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// サーバーとの接続が切れていることを表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disconnected;

/// メインスレッドが持つクライアントの操作口。
#[derive(Debug)]
pub struct ClientHandle {
    /// サーバーから割り当てられた自分のプレイヤー番号(ホストが0なので1以上)。
    /// `GameState::players` の添字と対応する。
    pub player_id: usize,

    /// サーバーから届いた最新のゲーム状態。
    /// 直接 `try_recv` してもよいが、溜まった分の畳み込みと切断判定まで面倒を見る
    /// [`ClientHandle::latest_snapshot`] を使うのが本来の入口。
    pub inbound: Receiver<Box<GameState>>,

    /// 自分の入力の送信口。[`ClientHandle::send`] 経由で使う。
    pub outbound: Sender<Action>,
}

impl ClientHandle {
    /// 自分の入力をサーバーへ送る。
    ///
    /// 入力が無いtick(`Action::None`)は送る必要がない(サーバー側は
    /// 「届いた有意な入力だけ」を次のtickで消費する作りになっている)ので、
    /// 呼び出し側は `Action::None` を弾いてから呼ぶとよい。
    pub fn send(&self, action: Action) -> Result<(), Disconnected> {
        self.outbound.send(action).map_err(|_| Disconnected)
    }

    /// 届いているスナップショットのうち最新のものを取り出す。
    ///
    /// 描画は最新状態だけが必要で、遅れて溜まった古い状態を順に描く意味はないため
    /// 途中のものは捨てる。まだ何も届いていなければ `Ok(None)`。
    pub fn latest_snapshot(&self) -> Result<Option<Box<GameState>>, Disconnected> {
        let mut latest = None;
        loop {
            match self.inbound.try_recv() {
                Ok(state) => latest = Some(state),
                Err(TryRecvError::Empty) => return Ok(latest),
                // 切断済みでも、まだ取り出せていない状態があればそれを先に返す。
                Err(TryRecvError::Disconnected) => {
                    return latest.map(Some).ok_or(Disconnected);
                }
            }
        }
    }
}

/// サーバーへ接続し、専用スレッドで送受信を回す。
///
/// `addr` は `host:port` 形式。`Welcome` でプレイヤー番号を受け取るまで
/// この関数の中で待つので、戻った時点で [`ClientHandle::player_id`] は確定している。
/// 接続失敗はTUIを起動する前にそのまま `Err` として受け取れる。
pub fn spawn(addr: &str) -> io::Result<ClientHandle> {
    let addr = addr.to_string();
    let (welcome_tx, welcome_rx) = mpsc::channel::<io::Result<usize>>();
    let (inbound_tx, inbound_rx) = mpsc::channel::<Box<GameState>>();
    let (outbound_tx, outbound_rx) = mpsc::channel::<Action>();

    thread::Builder::new()
        .name("bombermanterm-client".to_string())
        .spawn(move || {
            let runtime = match Runtime::new() {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = welcome_tx.send(Err(err));
                    return;
                }
            };

            let connected = runtime.block_on(connect(&addr));
            let (stream, player_id) = match connected {
                Ok(pair) => pair,
                Err(err) => {
                    let _ = welcome_tx.send(Err(err));
                    return;
                }
            };

            // 呼び出し側が待つのをやめていたら、接続を維持する意味は無い。
            if welcome_tx.send(Ok(player_id)).is_err() {
                return;
            }

            runtime.block_on(run(stream, inbound_tx, outbound_rx));
            // ブロッキング中継タスクの終了を待たずにスレッドを畳む
            // (中継は呼び出し側が送信口を捨てた時点で自然に終わる)。
            runtime.shutdown_background();
        })?;

    // スレッド内でも接続にタイムアウトを掛けているので、こちらは
    // 「スレッドが応答しない」ケースだけを拾う少し長めの待ちにする。
    match welcome_rx.recv_timeout(CONNECT_TIMEOUT + Duration::from_secs(2)) {
        Ok(Ok(player_id)) => Ok(ClientHandle {
            player_id,
            inbound: inbound_rx,
            outbound: outbound_tx,
        }),
        Ok(Err(err)) => Err(err),
        Err(RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for the server handshake",
        )),
        Err(RecvTimeoutError::Disconnected) => Err(io::Error::other(
            "the client thread stopped before the handshake completed",
        )),
    }
}

/// 接続して `Welcome` を受け取るまでのハンドシェイク。
async fn connect(addr: &str) -> io::Result<(TcpStream, usize)> {
    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out connecting to {addr}"),
            )
        })??;
    // 対戦ゲームなので、まとめ送りより即時性を優先する。
    let _ = stream.set_nodelay(true);

    let welcome = tokio::time::timeout(CONNECT_TIMEOUT, protocol::read_frame(&mut stream))
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for the welcome message",
            )
        })??;

    match welcome {
        ServerMessage::Welcome { player_id } => Ok((stream, player_id)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected a welcome message first, got {other:?}"),
        )),
    }
}

/// 送信(入力)と受信(スナップショット)を回す本体。サーバーが切れたら戻る。
async fn run(stream: TcpStream, inbound_tx: Sender<Box<GameState>>, outbound_rx: Receiver<Action>) {
    let (mut reader, mut writer) = stream.into_split();

    // メインスレッド(同期)からの入力を非同期側へ橋渡しする。
    // `std::sync::mpsc::Receiver::recv` はブロッキングなので専用スレッドで受ける。
    let (action_tx, mut action_rx) = tokio::sync::mpsc::unbounded_channel::<Action>();
    tokio::task::spawn_blocking(move || {
        while let Ok(action) = outbound_rx.recv() {
            if action_tx.send(action).is_err() {
                break;
            }
        }
    });

    let writer_task = tokio::spawn(async move {
        while let Some(action) = action_rx.recv().await {
            if protocol::write_message(&mut writer, &ClientMessage::Input(action))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    while let Ok(message) = protocol::read_frame::<_, ServerMessage>(&mut reader).await {
        match message {
            ServerMessage::Snapshot(state) => {
                // メインスレッドが受信口を捨てた(ゲーム終了)なら続ける意味は無い。
                if inbound_tx.send(state).is_err() {
                    break;
                }
            }
            // 2度目以降の Welcome は来ない想定。離脱通知は状態(players)にも
            // 反映されて届くので、描画のためだけに保持はしない。
            ServerMessage::Welcome { .. } | ServerMessage::PlayerLeft { .. } => continue,
        }
    }

    writer_task.abort();
    // ここで inbound_tx が落ちるので、メインスレッドは切断を検知できる。
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_reports_a_connection_error_instead_of_blocking_forever() {
        // 誰も待ち受けていないポートへの接続は、TUIを起動する前にエラーになる。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);

        let err = spawn(&addr.to_string()).expect_err("connection must fail");
        assert!(
            !err.to_string().is_empty(),
            "エラー理由が呼び出し側へ伝わること"
        );
    }

    #[test]
    fn latest_snapshot_keeps_only_the_newest_state_and_then_reports_disconnect() {
        let (inbound_tx, inbound_rx) = mpsc::channel::<Box<GameState>>();
        let (outbound_tx, outbound_rx) = mpsc::channel::<Action>();
        let handle = ClientHandle {
            player_id: 2,
            inbound: inbound_rx,
            outbound: outbound_tx,
        };

        // GameState は PartialEq を持たないので、形だけ matches! で確認する。
        assert!(
            matches!(handle.latest_snapshot(), Ok(None)),
            "まだ何も届いていない"
        );

        let mut older = GameState::new_multiplayer(2);
        older.players[0].score = 100;
        let mut newer = GameState::new_multiplayer(2);
        newer.players[0].score = 200;
        inbound_tx.send(Box::new(older)).expect("send");
        inbound_tx.send(Box::new(newer)).expect("send");

        let latest = handle
            .latest_snapshot()
            .expect("not disconnected")
            .expect("a snapshot");
        assert_eq!(latest.players[0].score, 200, "古い状態は捨てて最新を返す");

        // 送信側(ネットワークスレッド)が落ちたら切断として観測される。
        drop(inbound_tx);
        assert!(matches!(handle.latest_snapshot(), Err(Disconnected)));

        // 送信もできなくなる。
        drop(outbound_rx);
        assert_eq!(handle.send(Action::PlaceBomb), Err(Disconnected));
    }
}
