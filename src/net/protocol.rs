//! ネットワーク対戦のメッセージ定義とフレーミング。
//!
//! フレーミングは「4byteのペイロード長(u32, リトルエンディアン) + JSON本体」の
//! 長さプレフィックス方式。TCPはバイトストリームなのでメッセージ境界が保証されず、
//! 受信側で「どこまでが1メッセージか」を知る必要があるため長さを前置きする。
//! 送受信の両方で `to_le_bytes` / `from_le_bytes` を使い、バイト順を揃える。
//!
//! シリアライズは `serde_json`。可読性・デバッグしやすさを優先しており、
//! 通信量が問題になれば同じ構造のまま `bincode` 等へ差し替えられる。

use std::io;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::game::state::GameState;
use crate::types::Action;

/// 1フレームで受け入れるペイロード長の上限(4MiB)。
///
/// 壊れた相手・想定外のプロトコルを話す相手から巨大な長さを送られたときに、
/// そのサイズのバッファを確保してしまわないための安全弁。実際のスナップショットは
/// 15x13マスのマップと数個のエンティティで数KB程度にしかならない。
pub const MAX_FRAME_LEN: usize = 4 * 1024 * 1024;

/// クライアント → サーバー のメッセージ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// そのtickの自分の入力。
    Input(Action),
}

/// サーバー → クライアント のメッセージ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    /// 接続直後に1度だけ送られる、自分に割り当てられたプレイヤー番号。
    /// 0はホスト自身なので、クライアントには必ず1以上が割り当たる。
    Welcome { player_id: usize },
    /// 毎tickブロードキャストされる最新のゲーム状態。
    ///
    /// `GameState` は他のバリアントに比べて大きいので `Box` に入れる
    /// (enum全体のサイズが最大バリアントに引きずられるのを避ける)。
    Snapshot(Box<GameState>),
    /// 他のプレイヤーが切断したことの通知。
    PlayerLeft { player_id: usize },
}

/// メッセージを「4byte長さプレフィックス + JSON本体」の1フレームへ変換する。
///
/// 同じフレームを複数の接続へ配る使い方(スナップショットのブロードキャスト)を
/// 想定して、書き込みではなくバイト列を返す形にしてある。
pub fn encode_frame<T: Serialize>(message: &T) -> io::Result<Vec<u8>> {
    let payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large to send: {} bytes", payload.len()),
        ));
    }

    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// JSONペイロード(長さプレフィックスを含まない)をメッセージへ戻す。
pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> io::Result<T> {
    serde_json::from_slice(payload).map_err(io::Error::other)
}

/// フレームを1つ読み取ってメッセージへ戻す。
///
/// 相手が接続を閉じた場合は `read_exact` が `UnexpectedEof` を返すので、
/// 呼び出し側はそれを切断として扱う。
pub async fn read_frame<R, T>(reader: &mut R) -> io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds the {MAX_FRAME_LEN} byte limit"),
        ));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    decode_payload(&payload)
}

/// [`encode_frame`] で作ったフレームを書き出す。
pub async fn write_frame<W>(writer: &mut W, frame: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(frame).await?;
    writer.flush().await
}

/// メッセージを1つエンコードしてそのまま書き出す近道
/// (ブロードキャストしないハンドシェイク等の1対1のやり取り向け)。
pub async fn write_message<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let frame = encode_frame(message)?;
    write_frame(writer, &frame).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Direction, Screen};
    use std::io::Cursor;

    /// 検証に使う `Action` の全バリアント。
    /// 新しいバリアントが増えたらここにも足す(網羅漏れを機械的に気付けるよう
    /// `Action` を match して列挙する形にはせず、意図的に手で並べている)。
    const ALL_ACTIONS: [Action; 11] = [
        Action::Move(Direction::Up),
        Action::Move(Direction::Down),
        Action::Move(Direction::Left),
        Action::Move(Direction::Right),
        Action::PlaceBomb,
        Action::Pause,
        Action::Quit,
        Action::ToggleGodMode,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::None,
    ];

    #[test]
    fn frame_layout_is_a_little_endian_length_prefix_plus_json() {
        let frame = encode_frame(&ClientMessage::Input(Action::PlaceBomb)).expect("encode");

        let len = u32::from_le_bytes(frame[..4].try_into().expect("4 bytes")) as usize;
        assert_eq!(len, frame.len() - 4, "prefix must be the payload length");

        let payload = std::str::from_utf8(&frame[4..]).expect("payload is utf-8 json");
        assert!(
            payload.contains("PlaceBomb"),
            "payload must be readable json: {payload}"
        );
    }

    #[test]
    fn client_message_round_trips_for_every_action() {
        for action in ALL_ACTIONS {
            let message = ClientMessage::Input(action);
            let frame = encode_frame(&message).expect("encode");
            let decoded: ClientMessage = decode_payload(&frame[4..]).expect("decode");
            assert_eq!(decoded, message, "round trip must preserve {action:?}");
        }
    }

    #[test]
    fn server_welcome_and_player_left_round_trip() {
        let frame = encode_frame(&ServerMessage::Welcome { player_id: 3 }).expect("encode");
        let decoded: ServerMessage = decode_payload(&frame[4..]).expect("decode");
        assert!(matches!(decoded, ServerMessage::Welcome { player_id: 3 }));

        let frame = encode_frame(&ServerMessage::PlayerLeft { player_id: 2 }).expect("encode");
        let decoded: ServerMessage = decode_payload(&frame[4..]).expect("decode");
        assert!(matches!(
            decoded,
            ServerMessage::PlayerLeft { player_id: 2 }
        ));
    }

    #[test]
    fn snapshot_round_trip_preserves_the_game_state() {
        let mut state = GameState::new_multiplayer(4);
        state.screen = Screen::Playing;
        state.players[1].score = 4321;
        state.players[2].alive = false;
        state.players[3].invincible_remaining = 2.5;

        let frame =
            encode_frame(&ServerMessage::Snapshot(Box::new(state.clone()))).expect("encode");
        let decoded: ServerMessage = decode_payload(&frame[4..]).expect("decode");

        let ServerMessage::Snapshot(restored) = decoded else {
            panic!("expected a snapshot");
        };

        assert_eq!(restored.screen, state.screen);
        assert_eq!(restored.players.len(), state.players.len());
        assert_eq!(restored.players[1].score, 4321);
        assert!(!restored.players[2].alive);
        assert_eq!(restored.players[3].invincible_remaining, 2.5);
        assert_eq!(restored.map.width, state.map.width);
        assert_eq!(restored.map.height, state.map.height);

        // マップのタイルが1マスも欠けずに復元されていること。
        for row in 0..state.map.height as i32 {
            for col in 0..state.map.width as i32 {
                assert_eq!(
                    restored.map.tile_at((row, col)),
                    state.map.tile_at((row, col)),
                    "tile ({row}, {col}) must survive the round trip"
                );
            }
        }
    }

    #[tokio::test]
    async fn read_frame_reads_concatenated_frames_in_order() {
        let mut stream = Vec::new();
        for action in [Action::PlaceBomb, Action::Move(Direction::Left)] {
            stream.extend_from_slice(&encode_frame(&ClientMessage::Input(action)).expect("encode"));
        }

        let mut reader = Cursor::new(stream);
        let first: ClientMessage = read_frame(&mut reader).await.expect("first frame");
        let second: ClientMessage = read_frame(&mut reader).await.expect("second frame");

        assert_eq!(first, ClientMessage::Input(Action::PlaceBomb));
        assert_eq!(second, ClientMessage::Input(Action::Move(Direction::Left)));

        // 3つ目は無いので、末尾では切断(EOF)として観測される。
        let err = read_frame::<_, ClientMessage>(&mut reader)
            .await
            .expect_err("no third frame");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn write_frame_output_can_be_read_back() {
        let mut buffer: Vec<u8> = Vec::new();
        write_message(&mut buffer, &ServerMessage::Welcome { player_id: 1 })
            .await
            .expect("write");

        let mut reader = Cursor::new(buffer);
        let decoded: ServerMessage = read_frame(&mut reader).await.expect("read");
        assert!(matches!(decoded, ServerMessage::Welcome { player_id: 1 }));
    }

    #[tokio::test]
    async fn read_frame_rejects_an_oversized_length_header() {
        let mut stream = ((MAX_FRAME_LEN + 1) as u32).to_le_bytes().to_vec();
        stream.extend_from_slice(b"{}");

        let mut reader = Cursor::new(stream);
        let err = read_frame::<_, ClientMessage>(&mut reader)
            .await
            .expect_err("oversized frames must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_frame_rejects_a_malformed_payload() {
        let mut stream = 2u32.to_le_bytes().to_vec();
        stream.extend_from_slice(b"[]");

        let mut reader = Cursor::new(stream);
        assert!(
            read_frame::<_, ClientMessage>(&mut reader).await.is_err(),
            "json that is not a ClientMessage must be an error"
        );
    }
}
