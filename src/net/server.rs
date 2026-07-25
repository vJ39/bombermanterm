//! ネットワーク対戦のサーバー(ホスト)側。
//!
//! 役割分担(重要):
//! - **ゲームロジックはメインスレッド([`crate::main`])が回す**。この
//!   モジュールは「クライアントの入力を集めてメインスレッドへ渡す」
//!   「メインスレッドが作った最新状態を全クライアントへ配る」中継役に徹する。
//!   「サーバーが権威を持つ」という設計は、ホストのメインループだけが
//!   [`GameState`] を更新し、クライアントはそれを受け取って描画するだけ、
//!   という形で満たしている。
//! - tokio(非同期)はこのモジュールが起動する専用スレッドの中だけで動く。
//!   メインスレッドの同期ループとは `std::sync::mpsc` で橋渡しするので、
//!   既存のメインループを非同期化する必要はない。
//!
//! スレッド構成:
//! ```text
//!  メインスレッド(同期)                    サーバースレッド(tokio)
//!    latest_client_inputs() <── mpsc ── 入力収集タスク ←── 各接続の読み取りタスク
//!    send_snapshot(&state)  ──> mpsc ──> 中継タスク ──> broadcast ──> 各接続の書き込みタスク
//! ```

use std::io;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::sync::{broadcast, Notify};
use tokio::time::MissedTickBehavior;

use crate::game::state::{GameState, MAX_PLAYERS};
use crate::net::protocol::{self, ClientMessage, ServerMessage};
use crate::net::INPUT_COLLECT_INTERVAL;
use crate::types::Action;

/// メインスレッドからネットワークスレッドへ渡す、エンコード済みのスナップショット。
///
/// 全クライアントへ同じバイト列を配るので、接続ごとに作り直さず `Arc` で共有する
/// (JSONへの変換もメインスレッドで1回だけ行う)。
pub type SnapshotFrame = Arc<Vec<u8>>;

/// ブロードキャストチャンネルが保持するスナップショットの本数。
///
/// 書き込みが詰まったクライアントはこの本数を超えた分を取りこぼす(`Lagged`)が、
/// 対戦ゲームでは古い状態を無理に送るより最新へ追いつく方が正しいので、
/// 取りこぼしはそのまま捨てる。
const BROADCAST_CAPACITY: usize = 8;

/// メインスレッドが持つサーバーの操作口。
#[derive(Debug)]
pub struct ServerHandle {
    /// クライアントの最新入力セット。
    ///
    /// 1要素の `Vec<Action>` は常に長さ [`MAX_PLAYERS`] で、添字がプレイヤー番号。
    /// **添字0はホスト自身の予約枠で、ここには常に `Action::None` が入る**
    /// (メインスレッドが自分のキーボード入力で上書きしてから
    /// `GameState::tick_multi` へ渡す)。添字1以降は各クライアントの入力で、
    /// 未接続・切断済み・そのtickに入力が無かったクライアントは `Action::None`。
    ///
    /// 直接 `try_recv` してもよいが、溜まった複数セットの畳み込みまで面倒を見る
    /// [`ServerHandle::latest_client_inputs`] を使うのが本来の入口。
    pub inbound: Receiver<Vec<Action>>,

    /// メインスレッドが `tick_multi` 後の状態を渡す送信口。
    /// エンコードまで面倒を見る [`ServerHandle::send_snapshot`] 経由で使う。
    pub outbound: Sender<SnapshotFrame>,

    /// 接続中のクライアント数(ホスト自身は含まない)。
    client_count: Arc<AtomicUsize>,

    /// 実際に待ち受けているアドレス。`port` に0を渡した場合はOSが選んだ番号が入る。
    local_addr: SocketAddr,
}

impl ServerHandle {
    /// 溜まっているクライアント入力セットを全部取り込み、1セットへ畳んで返す。
    ///
    /// 畳み込みは添字ごとに「最後に届いた `Action::None` 以外の入力」を採用する。
    /// 最新の1セットだけ見て残りを捨てると、直前に届いていた入力(移動やボム設置)を
    /// 取りこぼすため、単純な上書きにはしていない。
    ///
    /// 返り値の添字0は常に `Action::None`(ホストの予約枠)。まだ何も届いていない
    /// 場合は `None` を返す。
    pub fn latest_client_inputs(&self) -> Option<Vec<Action>> {
        let mut merged: Option<Vec<Action>> = None;

        // `try_recv` の `Err` は Empty(まだ無い)と Disconnected(サーバースレッドが
        // 終了済み)の2種類だが、どちらもこれ以上読めない点では同じなので区別せず抜ける。
        // サーバーが落ちてもホストは自分だけ遊べる状態を保つ。
        while let Ok(incoming) = self.inbound.try_recv() {
            match merged.as_mut() {
                None => merged = Some(incoming),
                Some(current) => {
                    for (slot, action) in current.iter_mut().zip(incoming) {
                        if !matches!(action, Action::None) {
                            *slot = action;
                        }
                    }
                }
            }
        }

        merged
    }

    /// 最新のゲーム状態を全クライアントへ配る。
    ///
    /// JSONへの変換はここ(メインスレッド)で1回だけ行い、接続ごとには
    /// 出来上がったバイト列を共有する。`GameState` を1つ複製するコストが乗るが、
    /// 15x13マスのマップと数個のエンティティなので30fpsでも問題にならない。
    pub fn send_snapshot(&self, state: &GameState) -> io::Result<()> {
        let frame = protocol::encode_frame(&ServerMessage::Snapshot(Box::new(state.clone())))?;
        self.outbound
            .send(Arc::new(frame))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "server thread has stopped"))
    }

    /// 現在の参加人数(ホスト自身の1人 + 接続中のクライアント数)。ロビー表示に使う。
    pub fn player_count(&self) -> usize {
        1 + self.connected_clients()
    }

    /// 接続中のクライアント数(ホスト自身は含まない)。
    pub fn connected_clients(&self) -> usize {
        self.client_count.load(Ordering::Relaxed)
    }

    /// 実際に待ち受けているアドレス。
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

/// サーバーを専用スレッドで起動する。
///
/// `port` に0を渡すとOSが空きポートを選ぶ([`ServerHandle::local_addr`]で確認できる)。
/// `max_clients` はホスト自身を除いた受け入れ人数で、[`MAX_PLAYERS`]`- 1` に丸める。
/// 上限を超えた接続は即座に閉じる。
///
/// bind はこの関数の中(呼び出し元スレッド)で同期的に行うので、ポートが使用中などの
/// 失敗はTUIを起動する前にそのまま `Err` として受け取れる。
pub fn spawn(port: u16, max_clients: usize) -> io::Result<ServerHandle> {
    let max_clients = max_clients.min(MAX_PLAYERS - 1);

    let listener = StdTcpListener::bind(("0.0.0.0", port))?;
    // tokio へ渡す前にノンブロッキングにしておく(from_std の前提)。
    listener.set_nonblocking(true)?;
    let local_addr = listener.local_addr()?;

    let (inbound_tx, inbound_rx) = mpsc::channel::<Vec<Action>>();
    let (outbound_tx, outbound_rx) = mpsc::channel::<SnapshotFrame>();
    let client_count = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(Mutex::new(Shared::new()));

    let thread_shared = Arc::clone(&shared);
    let thread_client_count = Arc::clone(&client_count);
    thread::Builder::new()
        .name("bombermanterm-server".to_string())
        .spawn(move || {
            let runtime = match Runtime::new() {
                Ok(runtime) => runtime,
                Err(err) => {
                    eprintln!("failed to start the server runtime: {err}");
                    return;
                }
            };
            runtime.block_on(serve(
                listener,
                max_clients,
                thread_shared,
                thread_client_count,
                inbound_tx,
                outbound_rx,
            ));
        })?;

    Ok(ServerHandle {
        inbound: inbound_rx,
        outbound: outbound_tx,
        client_count,
        local_addr,
    })
}

/// クライアントごとの「まだメインスレッドへ渡していない入力」と接続状況。
///
/// 短い区間しかロックしない(`.await` を挟まない)ので `std::sync::Mutex` を使う。
struct Shared {
    /// 未消費の入力。添字がプレイヤー番号で、添字0はホストの枠なので常に未使用。
    pending: [Action; MAX_PLAYERS],
    /// 接続中のプレイヤー番号。
    connected: [bool; MAX_PLAYERS],
}

impl Shared {
    fn new() -> Self {
        Self {
            pending: [Action::None; MAX_PLAYERS],
            connected: [false; MAX_PLAYERS],
        }
    }

    /// 空いているプレイヤー番号を1つ確保する。満員なら `None`。
    /// 0はホスト自身の枠なので必ず1から探す。
    fn claim_slot(&mut self, max_clients: usize) -> Option<usize> {
        let last = max_clients.min(MAX_PLAYERS - 1);
        (1..=last).find(|&id| !self.connected[id]).inspect(|&id| {
            self.connected[id] = true;
            self.pending[id] = Action::None;
        })
    }

    /// クライアントの入力を記録する。
    ///
    /// `Action::None`(入力なし)では既存の値を上書きしない。クライアントは
    /// 有意な入力だけを送るが、仮に空入力が届いても、まだメインスレッドが
    /// 拾っていない移動・設置を消してしまわないようにする。
    fn record_input(&mut self, player_id: usize, action: Action) {
        if matches!(action, Action::None) {
            return;
        }
        if self.connected.get(player_id).copied() != Some(true) {
            return;
        }
        self.pending[player_id] = action;
    }

    /// 未消費の入力を取り出し、枠を空にする。
    fn take_pending(&mut self) -> Vec<Action> {
        let taken = self.pending.to_vec();
        self.pending = [Action::None; MAX_PLAYERS];
        taken
    }

    fn disconnect(&mut self, player_id: usize) {
        if player_id < MAX_PLAYERS {
            self.connected[player_id] = false;
            self.pending[player_id] = Action::None;
        }
    }
}

/// サーバースレッド上のtokioランタイムで動く本体。
async fn serve(
    listener: StdTcpListener,
    max_clients: usize,
    shared: Arc<Mutex<Shared>>,
    client_count: Arc<AtomicUsize>,
    inbound_tx: Sender<Vec<Action>>,
    outbound_rx: Receiver<SnapshotFrame>,
) {
    let listener = match TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to hand the listener over to tokio: {err}");
            return;
        }
    };

    let (broadcast_tx, _) = broadcast::channel::<SnapshotFrame>(BROADCAST_CAPACITY);
    // メインスレッドが終了(= ServerHandle が破棄)したことを検知したら、
    // accept ループも畳んでスレッドごと終わらせるための合図。
    let shutdown = Arc::new(Notify::new());

    spawn_snapshot_bridge(outbound_rx, broadcast_tx.clone());
    tokio::spawn(collect_inputs(
        Arc::clone(&shared),
        inbound_tx,
        Arc::clone(&shutdown),
    ));

    loop {
        let accepted = tokio::select! {
            result = listener.accept() => result,
            _ = shutdown.notified() => break,
        };

        let stream = match accepted {
            Ok((stream, _peer)) => stream,
            Err(err) => {
                // 1接続の失敗でサーバー全体を止めない。
                eprintln!("failed to accept a client: {err}");
                continue;
            }
        };

        let Some(player_id) = shared.lock().expect("server state lock").claim_slot(max_clients)
        else {
            // 満員。理由の通知は行わず接続を閉じるだけに留める(v2の範囲外)。
            drop(stream);
            continue;
        };
        client_count.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(handle_client(
            stream,
            player_id,
            Arc::clone(&shared),
            Arc::clone(&client_count),
            broadcast_tx.clone(),
        ));
    }
}

/// メインスレッド(同期)からのスナップショットを、非同期側の
/// ブロードキャストチャンネルへ流し込む中継。
///
/// `std::sync::mpsc::Receiver::recv` はブロッキングなので、非同期タスクの実行を
/// 止めないように専用のブロッキングスレッドで回す。
fn spawn_snapshot_bridge(
    outbound_rx: Receiver<SnapshotFrame>,
    broadcast_tx: broadcast::Sender<SnapshotFrame>,
) {
    tokio::task::spawn_blocking(move || {
        while let Ok(frame) = outbound_rx.recv() {
            // 購読者(クライアント)が居ないときのエラーは正常な状態なので無視する。
            let _ = broadcast_tx.send(frame);
        }
    });
}

/// 一定間隔でクライアントの入力をまとめてメインスレッドへ渡す。
///
/// 入力が無いtickでも送る。メインスレッド側が受信口を捨てた(ゲーム終了)ことを
/// 送信エラーとして検知し、サーバーを畳むきっかけにするため。
async fn collect_inputs(
    shared: Arc<Mutex<Shared>>,
    inbound_tx: Sender<Vec<Action>>,
    shutdown: Arc<Notify>,
) {
    let mut ticker = tokio::time::interval(INPUT_COLLECT_INTERVAL);
    // 遅れた分を早回しで取り戻すと入力が一気に流れてしまうので、素直に遅らせる。
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        let inputs = shared.lock().expect("server state lock").take_pending();
        if inbound_tx.send(inputs).is_err() {
            // `notify_waiters` は待っている最中のタスクにしか届かず、accept ループが
            // ちょうど接続処理中だと取りこぼす。`notify_one` は通知を1つ保持するので、
            // 次に `notified()` を待った時点で確実に受け取れる。
            shutdown.notify_one();
            return;
        }
    }
}

/// 1クライアントとのやり取り。
///
/// 読み取り(入力の受信)をこのタスクで行い、書き込み(スナップショットの送信)は
/// 別タスクへ分ける。片方が詰まってももう片方が止まらないようにするため。
async fn handle_client(
    stream: TcpStream,
    player_id: usize,
    shared: Arc<Mutex<Shared>>,
    client_count: Arc<AtomicUsize>,
    broadcast_tx: broadcast::Sender<SnapshotFrame>,
) {
    // 対戦ゲームなので、まとめ送りより即時性を優先する。
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();

    if protocol::write_message(&mut writer, &ServerMessage::Welcome { player_id })
        .await
        .is_err()
    {
        release_slot(&shared, &client_count, player_id);
        return;
    }

    let mut snapshots = broadcast_tx.subscribe();
    let writer_task = tokio::spawn(async move {
        loop {
            match snapshots.recv().await {
                Ok(frame) => {
                    if protocol::write_frame(&mut writer, &frame).await.is_err() {
                        break;
                    }
                }
                // 書き込みが遅れて溢れた分は捨てて最新へ追いつく。
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // 相手が切断する(EOF)か、壊れたメッセージが来るまで入力を読み続ける。
    while let Ok(message) = protocol::read_frame::<_, ClientMessage>(&mut reader).await {
        match message {
            ClientMessage::Input(action) => shared
                .lock()
                .expect("server state lock")
                .record_input(player_id, action),
        }
    }

    writer_task.abort();
    release_slot(&shared, &client_count, player_id);
    // 残っているクライアントへ離脱を伝える。エンコードに失敗しても致命的ではない。
    if let Ok(frame) = protocol::encode_frame(&ServerMessage::PlayerLeft { player_id }) {
        let _ = broadcast_tx.send(Arc::new(frame));
    }
}

/// 切断したクライアントのプレイヤー枠を解放する。
fn release_slot(shared: &Mutex<Shared>, client_count: &AtomicUsize, player_id: usize) {
    shared.lock().expect("server state lock").disconnect(player_id);
    // 二重に減らして 0 を下回らないよう、接続中だったときだけ減算する。
    let _ = client_count.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        count.checked_sub(1)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Direction;

    #[test]
    fn claim_slot_never_hands_out_the_host_slot_and_respects_the_limit() {
        let mut shared = Shared::new();

        assert_eq!(shared.claim_slot(3), Some(1));
        assert_eq!(shared.claim_slot(3), Some(2));
        assert_eq!(shared.claim_slot(3), Some(3));
        assert_eq!(shared.claim_slot(3), None, "4人目のクライアントは入れない");

        // 途中の1人が抜けたら、その番号が再利用される。
        shared.disconnect(2);
        assert_eq!(shared.claim_slot(3), Some(2));
    }

    #[test]
    fn claim_slot_is_capped_by_max_players_even_if_asked_for_more() {
        let mut shared = Shared::new();
        let mut claimed = Vec::new();
        while let Some(id) = shared.claim_slot(99) {
            claimed.push(id);
        }
        assert_eq!(claimed, vec![1, 2, 3], "ホストの枠を含めて最大4人まで");
    }

    #[test]
    fn record_input_keeps_the_last_meaningful_action_per_player() {
        let mut shared = Shared::new();
        let id = shared.claim_slot(3).expect("slot");

        shared.record_input(id, Action::Move(Direction::Up));
        // 空入力では上書きしない(未消費の入力を消さない)。
        shared.record_input(id, Action::None);
        assert_eq!(shared.pending[id], Action::Move(Direction::Up));

        // 有意な入力なら上書きする。
        shared.record_input(id, Action::PlaceBomb);
        assert_eq!(shared.pending[id], Action::PlaceBomb);
    }

    #[test]
    fn record_input_ignores_players_that_are_not_connected() {
        let mut shared = Shared::new();
        shared.record_input(2, Action::PlaceBomb);
        assert_eq!(shared.pending[2], Action::None);
    }

    #[test]
    fn take_pending_clears_the_slots_and_leaves_the_host_slot_empty() {
        let mut shared = Shared::new();
        let id = shared.claim_slot(3).expect("slot");
        shared.record_input(id, Action::PlaceBomb);

        let taken = shared.take_pending();
        assert_eq!(taken.len(), MAX_PLAYERS);
        assert_eq!(taken[0], Action::None, "添字0はホスト用の予約枠");
        assert_eq!(taken[id], Action::PlaceBomb);

        // 一度取り出した入力は消える(同じ入力が次のtickでも効いてしまわない)。
        assert!(shared
            .take_pending()
            .iter()
            .all(|action| matches!(action, Action::None)));
    }

    #[test]
    fn disconnect_drops_pending_input() {
        let mut shared = Shared::new();
        let id = shared.claim_slot(3).expect("slot");
        shared.record_input(id, Action::PlaceBomb);

        shared.disconnect(id);
        assert_eq!(shared.pending[id], Action::None);
        // 切断後に届いた入力は無視される。
        shared.record_input(id, Action::PlaceBomb);
        assert_eq!(shared.pending[id], Action::None);
    }

    #[test]
    fn spawn_binds_a_port_and_reports_no_clients_yet() {
        let server = spawn(0, 3).expect("bind on an ephemeral port");
        assert_ne!(server.local_addr().port(), 0, "OSが選んだポートが分かる");
        assert_eq!(server.connected_clients(), 0);
        assert_eq!(server.player_count(), 1, "ホスト自身の1人だけ");
    }

    #[test]
    fn latest_client_inputs_merges_pending_sets_without_losing_actions() {
        let (tx, rx) = mpsc::channel::<Vec<Action>>();
        let handle = ServerHandle {
            inbound: rx,
            outbound: mpsc::channel().0,
            client_count: Arc::new(AtomicUsize::new(0)),
            local_addr: "127.0.0.1:0".parse().expect("addr"),
        };

        assert!(
            handle.latest_client_inputs().is_none(),
            "何も届いていなければ None"
        );

        // 先に届いた移動が、後から届いた空入力で消えないこと。
        let mut first = vec![Action::None; MAX_PLAYERS];
        first[1] = Action::Move(Direction::Left);
        first[2] = Action::PlaceBomb;
        tx.send(first).expect("send");

        let mut second = vec![Action::None; MAX_PLAYERS];
        second[2] = Action::Move(Direction::Up);
        tx.send(second).expect("send");

        let merged = handle.latest_client_inputs().expect("merged inputs");
        assert_eq!(merged[0], Action::None, "ホストの枠は空のまま");
        assert_eq!(merged[1], Action::Move(Direction::Left));
        assert_eq!(
            merged[2],
            Action::Move(Direction::Up),
            "同じプレイヤーの入力は新しい方を採用する"
        );

        // 取り込んだ分は消費済み。
        assert!(handle.latest_client_inputs().is_none());
    }
}
