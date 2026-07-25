//! rodio再生管理(BGM/SE)。
//!
//! `RodioPlayer` は、`synth` モジュールに実装した `rodio::Source` 自前実装
//! (矩形波/三角波/ノイズ合成)を組み合わせて、オリジナルのチップチューン風
//! BGM/SEを実行時にオンザフライで合成・再生する。本家音源・メロディは
//! 一切使用せず、wavファイル等の同梱も行わない。
//!
//! - BGMは `Player` 1本のみで管理する(常に高々1曲が鳴る)。切り替え時は
//!   既存の曲をクリアしてから新しい曲を積み直す。
//! - SEは共有 `Mixer` に都度ワンショットの `Source` を追加する方式で、
//!   複数SEの同時再生に対応する。
//!
//! 出力デバイスが取得できない環境(CI・ヘッドレス端末等)では
//! `RodioPlayer::new()` はパニックさせず、以降の再生操作は何もしない
//! (無音)フォールバックとして振る舞う。

mod synth;

use rodio::mixer::Mixer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player, Source};

use crate::types::{Bgm, SoundEffect};
use synth::{hz, Note, Sequencer, Waveform, A, B, C, D, E, F, G};

pub trait AudioPlayer {
    fn play_se(&mut self, se: SoundEffect);
    fn play_bgm(&mut self, bgm: Bgm);
    fn stop_bgm(&mut self);
}

/// 実デバイスに接続できたときだけ生成される内部ハンドル一式。
struct Output {
    // 保持しておかないとドロップ時にストリームごと再生が止まるため、
    // 参照はしないが生存させておくだけのフィールド。
    #[allow(dead_code)]
    device: MixerDeviceSink,
    se_mixer: Mixer,
    bgm_player: Player,
}

pub struct RodioPlayer {
    // デバイスが開けなかった場合は `None` にし、無音フォールバックとする。
    output: Option<Output>,
}

/// BGMの全体音量(SEより控えめにして、ループ再生で耳が疲れないようにする)。
const BGM_MASTER_VOLUME: f32 = 0.35;

impl RodioPlayer {
    pub fn new() -> Self {
        let output = match DeviceSinkBuilder::open_default_sink() {
            Ok(mut device) => {
                device.log_on_drop(false);
                let se_mixer = device.mixer().clone();
                let bgm_player = Player::connect_new(device.mixer());
                bgm_player.set_volume(BGM_MASTER_VOLUME);
                Some(Output {
                    device,
                    se_mixer,
                    bgm_player,
                })
            }
            Err(err) => {
                eprintln!("audio device unavailable, running muted: {err}");
                None
            }
        };
        RodioPlayer { output }
    }
}

impl Default for RodioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioPlayer for RodioPlayer {
    fn play_se(&mut self, se: SoundEffect) {
        let Some(output) = self.output.as_ref() else {
            return;
        };
        output.se_mixer.add(Sequencer::new(se_notes(se)));
    }

    fn play_bgm(&mut self, bgm: Bgm) {
        let Some(output) = self.output.as_mut() else {
            return;
        };

        // 常に高々1曲: 既存の曲をクリアしてから積み直す。
        output.bgm_player.clear();
        match bgm {
            Bgm::Title => {
                let lead = Sequencer::new(title_lead()).repeat_infinite();
                let bass = Sequencer::new(title_bass()).repeat_infinite();
                let drums = Sequencer::new(title_drums()).repeat_infinite();
                output.bgm_player.append(lead.mix(bass).mix(drums));
            }
            Bgm::Stage => {
                let lead = Sequencer::new(stage_lead()).repeat_infinite();
                let bass = Sequencer::new(stage_bass()).repeat_infinite();
                let drums = Sequencer::new(stage_drums()).repeat_infinite();
                output.bgm_player.append(lead.mix(bass).mix(drums));
            }
            Bgm::Clear => {
                output
                    .bgm_player
                    .append(Sequencer::new(clear_notes()).repeat_infinite());
            }
            Bgm::GameOver => {
                output
                    .bgm_player
                    .append(Sequencer::new(gameover_notes()).repeat_infinite());
            }
        }
        output.bgm_player.play();
    }

    fn stop_bgm(&mut self) {
        let Some(output) = self.output.as_ref() else {
            return;
        };
        output.bgm_player.clear();
    }
}

// --- BGM/SE のメロディ定義 -------------------------------------------------
//
// すべてオリジナルのチップチューン風フレーズ(本家メロディの流用なし)。
// `hz(音名, オクターブ)` は平均律(A4=440Hz)での周波数計算。

/// タイトル画面: 明るく軽快な待機ループ(リード+ベースの2声)。
/// Aメロ→サビの2部構成で、サビは一段高いキー・大きい音量で山場を作る。
fn title_lead() -> Vec<Note> {
    let mut notes = title_verse_lead();
    notes.extend(title_chorus_lead());
    notes
}

fn title_bass() -> Vec<Note> {
    let mut notes = title_verse_bass();
    notes.extend(title_chorus_bass());
    notes
}

fn title_verse_lead() -> Vec<Note> {
    use Waveform::Square;
    const STEP: f32 = 0.16;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Square, 0.32);
    vec![
        n(C, 5),
        n(E, 5),
        n(G, 5),
        n(C, 6),
        n(A, 5),
        n(G, 5),
        n(E, 5),
        n(C, 5),
        n(D, 5),
        n(F, 5),
        n(A, 5),
        n(D, 6),
        n(C, 5),
        n(B, 4),
        n(G, 4),
        n(C, 5),
    ]
}

fn title_verse_bass() -> Vec<Note> {
    use Waveform::Triangle;
    const STEP: f32 = 0.64;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Triangle, 0.28);
    vec![n(C, 3), n(G, 3), n(A, 3), n(F, 3)]
}

/// タイトルBGMのサビ: ベロシティを上げ、Aメロより高いオクターブで開放感を出す。
fn title_chorus_lead() -> Vec<Note> {
    use Waveform::Square;
    const STEP: f32 = 0.16;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Square, 0.40);
    vec![
        n(E, 5),
        n(G, 5),
        n(C, 6),
        n(E, 6),
        n(D, 6),
        n(C, 6),
        n(G, 5),
        n(C, 6),
    ]
}

fn title_chorus_bass() -> Vec<Note> {
    use Waveform::Triangle;
    const STEP: f32 = 0.64;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Triangle, 0.32);
    vec![n(F, 3), n(G, 3)]
}

/// タイトルBGMの控えめなハイハット(8分音符)。本家のような軽快な弾みを
/// 待機ループにも少しだけ加える。verse/chorus合わせて `title_lead()` と
/// 同じ長さになるよう、0.08秒刻み×(32+16)個で組む。
fn title_drums() -> Vec<Note> {
    let mut notes = hi_hat_pattern(32, 0.08, 0.10, 0.05);
    notes.extend(hi_hat_pattern(16, 0.08, 0.10, 0.05));
    notes
}

/// ステージBGM: 走り回る緊張感のある駆け足ループ(リード+ベースの2声)。
/// Aメロ→サビの2部構成で、サビはより高いキー・大きい音量で盛り上げる。
fn stage_lead() -> Vec<Note> {
    let mut notes = stage_verse_lead();
    notes.extend(stage_chorus_lead());
    notes
}

fn stage_bass() -> Vec<Note> {
    let mut notes = stage_verse_bass();
    notes.extend(stage_chorus_bass());
    notes
}

fn stage_verse_lead() -> Vec<Note> {
    use Waveform::Square;
    const STEP: f32 = 0.14;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Square, 0.30);
    vec![
        n(A, 4),
        n(A, 4),
        n(C, 5),
        n(A, 4),
        n(E, 5),
        n(A, 4),
        n(C, 5),
        n(A, 4),
        n(D, 5),
        n(D, 5),
        n(F, 5),
        n(D, 5),
        n(C, 5),
        n(A, 4),
        n(G, 4),
        n(E, 4),
    ]
}

fn stage_verse_bass() -> Vec<Note> {
    use Waveform::Triangle;
    const STEP: f32 = 0.56;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Triangle, 0.32);
    vec![n(A, 3), n(A, 3), n(D, 3), n(C, 3)]
}

/// ステージBGMのサビ: Aメロより1オクターブ張ったリードと、動きのあるベースで
/// 駆け足感の山場を作る。
fn stage_chorus_lead() -> Vec<Note> {
    use Waveform::Square;
    const STEP: f32 = 0.14;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Square, 0.38);
    vec![
        n(D, 5),
        n(D, 5),
        n(F, 5),
        n(A, 5),
        n(D, 6),
        n(D, 6),
        n(A, 5),
        n(F, 5),
    ]
}

fn stage_chorus_bass() -> Vec<Note> {
    use Waveform::Triangle;
    const STEP: f32 = 0.28;
    let n = |name: i32, oct: i32| Note::tone(hz(name, oct), STEP, Triangle, 0.34);
    vec![n(D, 3), n(F, 3), n(G, 3), n(A, 3)]
}

/// ステージBGMのドラムパート。本家のような走り回る軽快さを出す本命の要素で、
/// 8分音符のハイハットに2拍ごとスネア風のアクセントを挟む。
/// verse(32個)+chorus(16個)を0.07秒刻みで組み、`stage_lead()`と同じ長さにする。
fn stage_drums() -> Vec<Note> {
    let mut notes = snare_accented_hat_pattern(32, 0.07, 0.18, 0.09);
    notes.extend(snare_accented_hat_pattern(16, 0.07, 0.20, 0.10));
    notes
}

/// シンプルな8分音符ハイハット(表拍を強め、裏拍を弱めに)を `count` 個刻む。
fn hi_hat_pattern(count: usize, step: f32, strong_vol: f32, weak_vol: f32) -> Vec<Note> {
    use Waveform::Noise;
    (0..count)
        .map(|i| {
            let vol = if i % 2 == 0 { strong_vol } else { weak_vol };
            Note::tone(5200.0, step, Noise, vol)
        })
        .collect()
}

/// ハイハットの合間に2拍ごと(4ステップごと)スネア風の低めのノイズを挟むパターン。
fn snare_accented_hat_pattern(count: usize, step: f32, hat_vol: f32, snare_vol: f32) -> Vec<Note> {
    use Waveform::Noise;
    (0..count)
        .map(|i| {
            if i % 4 == 2 {
                Note::tone(1400.0, step, Noise, snare_vol)
            } else {
                let vol = if i % 2 == 0 { hat_vol } else { hat_vol * 0.5 };
                Note::tone(5200.0, step, Noise, vol)
            }
        })
        .collect()
}

/// ステージクリアBGM: 短い勝利のファンファーレの繰り返し。
fn clear_notes() -> Vec<Note> {
    use Waveform::Square;
    let n = |name: i32, oct: i32, dur: f32| Note::tone(hz(name, oct), dur, Square, 0.42);
    vec![
        n(C, 5, 0.12),
        n(E, 5, 0.12),
        n(G, 5, 0.12),
        n(C, 6, 0.24),
        n(G, 5, 0.12),
        n(C, 6, 0.36),
        Note::rest(0.40),
    ]
}

/// ゲームオーバーBGM: しんみりと下降していく短調のフレーズの繰り返し。
fn gameover_notes() -> Vec<Note> {
    use Waveform::Triangle;
    let n = |name: i32, oct: i32, dur: f32| Note::tone(hz(name, oct), dur, Triangle, 0.30);
    vec![
        n(A, 4, 0.30),
        n(G, 4, 0.30),
        n(F, 4, 0.30),
        n(E, 4, 0.45),
        n(D, 4, 0.30),
        n(C, 4, 0.60),
        Note::rest(0.60),
    ]
}

fn se_notes(se: SoundEffect) -> Vec<Note> {
    use Waveform::{Noise, Square};
    match se {
        SoundEffect::PlaceBomb => vec![
            Note::tone(hz(G, 3), 0.04, Square, 0.55),
            Note::tone(hz(D, 3), 0.05, Square, 0.50),
            Note::tone(hz(G, 2), 0.07, Square, 0.45),
        ],
        SoundEffect::Explosion => vec![
            Note::tone(2600.0, 0.05, Noise, 0.90),
            Note::tone(1600.0, 0.07, Noise, 0.75),
            Note::tone(900.0, 0.09, Noise, 0.55),
            Note::tone(450.0, 0.14, Noise, 0.35),
            // 本家の爆発音を意識した低音の余韻(ドン、と尾を引く感じ)。
            Note::tone(140.0, 0.20, Noise, 0.30),
        ],
        SoundEffect::ItemGet => vec![
            Note::tone(hz(C, 5), 0.04, Square, 0.42),
            Note::tone(hz(E, 5), 0.04, Square, 0.42),
            Note::tone(hz(G, 5), 0.04, Square, 0.42),
            Note::tone(hz(C, 6), 0.07, Square, 0.45),
        ],
        SoundEffect::Death => vec![
            Note::tone(hz(A, 4), 0.08, Square, 0.50),
            Note::tone(hz(F, 4), 0.08, Square, 0.48),
            Note::tone(hz(D, 4), 0.10, Square, 0.46),
            Note::tone(hz(A, 3), 0.16, Square, 0.42),
            Note::tone(220.0, 0.09, Noise, 0.28),
        ],
        SoundEffect::StageClear => vec![
            Note::tone(hz(C, 5), 0.06, Square, 0.40),
            Note::tone(hz(E, 5), 0.06, Square, 0.40),
            Note::tone(hz(G, 5), 0.06, Square, 0.40),
            Note::tone(hz(C, 6), 0.06, Square, 0.42),
            Note::tone(hz(E, 6), 0.06, Square, 0.42),
            Note::tone(hz(G, 6), 0.16, Square, 0.45),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_SOUND_EFFECTS: [SoundEffect; 5] = [
        SoundEffect::PlaceBomb,
        SoundEffect::Explosion,
        SoundEffect::ItemGet,
        SoundEffect::Death,
        SoundEffect::StageClear,
    ];

    const ALL_BGM: [Bgm; 4] = [Bgm::Title, Bgm::Stage, Bgm::Clear, Bgm::GameOver];

    fn total_secs(notes: Vec<Note>) -> f32 {
        Sequencer::new(notes)
            .total_duration()
            .expect("Sequencer always reports a finite duration")
            .as_secs_f32()
    }

    #[test]
    fn title_lead_and_bass_loop_lengths_match() {
        let lead = total_secs(title_lead());
        let bass = total_secs(title_bass());
        assert!(
            (lead - bass).abs() < 0.01,
            "lead={lead} bass={bass} must match so the loop seam is clean"
        );
    }

    #[test]
    fn stage_lead_and_bass_loop_lengths_match() {
        let lead = total_secs(stage_lead());
        let bass = total_secs(stage_bass());
        assert!(
            (lead - bass).abs() < 0.01,
            "lead={lead} bass={bass} must match so the loop seam is clean"
        );
    }

    #[test]
    fn title_drums_length_matches_lead() {
        let lead = total_secs(title_lead());
        let drums = total_secs(title_drums());
        assert!(
            (lead - drums).abs() < 0.01,
            "lead={lead} drums={drums} must match so the loop seam is clean"
        );
    }

    #[test]
    fn stage_drums_length_matches_lead() {
        let lead = total_secs(stage_lead());
        let drums = total_secs(stage_drums());
        assert!(
            (lead - drums).abs() < 0.01,
            "lead={lead} drums={drums} must match so the loop seam is clean"
        );
    }

    #[test]
    fn every_bgm_and_se_definition_is_non_empty() {
        for se in ALL_SOUND_EFFECTS {
            assert!(!se_notes(se).is_empty());
        }
        assert!(!clear_notes().is_empty());
        assert!(!gameover_notes().is_empty());
        assert!(!title_lead().is_empty());
        assert!(!title_bass().is_empty());
        assert!(!title_drums().is_empty());
        assert!(!stage_lead().is_empty());
        assert!(!stage_bass().is_empty());
        assert!(!stage_drums().is_empty());
    }

    /// 実デバイスの有無どちらでもパニックしないこと(CI/ヘッドレス環境向けの
    /// 無音フォールバックを含む)。
    #[test]
    fn rodio_player_lifecycle_never_panics() {
        let mut player = RodioPlayer::new();

        for bgm in ALL_BGM {
            player.play_bgm(bgm);
        }
        for se in ALL_SOUND_EFFECTS {
            player.play_se(se);
        }
        player.stop_bgm();
    }

    #[test]
    fn default_impl_matches_new() {
        let _player: RodioPlayer = Default::default();
    }
}
