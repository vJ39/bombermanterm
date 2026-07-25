//! 自前のチップチューン風シンセサイザー。
//!
//! `rodio::Source` を自前実装した矩形波・三角波・ノイズの3種の波形を、
//! 周波数/長さ/音量の数値定義(`Note`)の列として時間軸上に並べて演奏する
//! `Sequencer` を提供する。波形はすべて実行時にオンザフライで合成し、
//! wavファイル等の同梱・本家音源の流用は一切行わない。
//!
//! `Sequencer` はノート列を1回演奏したら終わる有限の `Source`。BGMとして
//! 無限ループさせたい場合は呼び出し側(`super::RodioPlayer`)で
//! `Source::repeat_infinite()` を使う。

use std::time::Duration;

use rodio::{nz, ChannelCount, Sample, SampleRate, Source};

/// 内部合成のサンプルレート。
const SAMPLE_RATE: SampleRate = nz!(44_100);

/// 基本波形の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Square,
    Triangle,
    /// 破裂音・爆発音向けの疑似ノイズ。`freq` はノイズの切り替わり速さ(粗さ)として扱う。
    Noise,
}

/// 1音符(休符含む)の定義。
#[derive(Debug, Clone, Copy)]
pub struct Note {
    /// 発音周波数(Hz)。休符は`None`。
    freq: Option<f32>,
    /// 発音長(秒)。
    duration: f32,
    waveform: Waveform,
    /// 0.0-1.0 の音量。
    volume: f32,
}

impl Note {
    /// 発音する音符を作る。
    pub fn tone(freq: f32, duration: f32, waveform: Waveform, volume: f32) -> Self {
        Note {
            freq: Some(freq),
            duration,
            waveform,
            volume,
        }
    }

    /// 休符を作る。
    pub fn rest(duration: f32) -> Self {
        Note {
            freq: None,
            duration,
            waveform: Waveform::Square,
            volume: 0.0,
        }
    }
}

/// A4(440Hz)を基準にした平均律の周波数計算。
///
/// `name` は同一オクターブ内でのA4からの半音差(下記定数参照)、
/// `octave` は科学的音名のオクターブ番号(A4のオクターブが4)。
pub fn hz(name: i32, octave: i32) -> f32 {
    let semitone = name + (octave - 4) * 12;
    440.0 * 2f32.powf(semitone as f32 / 12.0)
}

// A4からの半音差(オクターブ4内での相対位置)。
pub const C: i32 = -9;
pub const D: i32 = -7;
pub const E: i32 = -5;
pub const F: i32 = -4;
pub const G: i32 = -2;
pub const A: i32 = 0;
pub const B: i32 = 2;

fn square_wave(phase: f32) -> f32 {
    if phase < 0.5 {
        1.0
    } else {
        -1.0
    }
}

fn triangle_wave(phase: f32) -> f32 {
    if phase < 0.5 {
        4.0 * phase - 1.0
    } else {
        3.0 - 4.0 * phase
    }
}

/// 16bit LFSR による疑似ノイズビット(-1.0/1.0)。ファミコン風ノイズチャンネルの発想を
/// 自前実装したもの。
fn lfsr_bit(state: &mut u16) -> f32 {
    let feedback = (*state ^ (*state >> 1)) & 1;
    *state >>= 1;
    if feedback == 1 {
        *state |= 1 << 14;
    }
    if *state & 1 == 1 {
        1.0
    } else {
        -1.0
    }
}

/// ノート境界のプチノイズ(クリック)を避けるための単純なアタック/リリース。
fn envelope(pos_in_note: u32, note_len: u32) -> f32 {
    let by_len = (note_len / 8).max(1);
    let by_time = (SAMPLE_RATE.get() as f32 * 0.006) as u32;
    let fade = by_len.min(by_time).max(1);

    if pos_in_note < fade {
        pos_in_note as f32 / fade as f32
    } else if pos_in_note + fade >= note_len {
        note_len.saturating_sub(pos_in_note) as f32 / fade as f32
    } else {
        1.0
    }
}

/// 音符列を1回だけ演奏する `rodio::Source`。
///
/// ループはさせない設計で、末尾まで再生し終えると `Iterator::next` が
/// `None` を返して終了する。BGMのループ再生は呼び出し側で
/// `.repeat_infinite()` を適用する。
pub struct Sequencer {
    notes: Vec<Note>,
    total_duration: Duration,
    idx: usize,
    pos_in_note: u32,
    phase: f32,
    noise_state: u16,
    noise_hold: f32,
}

impl Sequencer {
    pub fn new(notes: Vec<Note>) -> Self {
        let total_secs: f32 = notes.iter().map(|n| n.duration.max(0.0)).sum();
        Sequencer {
            notes,
            total_duration: Duration::from_secs_f32(total_secs),
            idx: 0,
            pos_in_note: 0,
            phase: 0.0,
            // 適当な非ゼロ初期状態(0だとLFSRが動かなくなる)。
            noise_state: 0xACE1,
            noise_hold: 0.0,
        }
    }

    fn note_len_samples(note: &Note) -> u32 {
        ((note.duration.max(0.0) * SAMPLE_RATE.get() as f32).round() as u32).max(1)
    }
}

impl Iterator for Sequencer {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        loop {
            let note = *self.notes.get(self.idx)?;
            let note_len = Self::note_len_samples(&note);

            if self.pos_in_note >= note_len {
                self.idx += 1;
                self.pos_in_note = 0;
                self.phase = 0.0;
                continue;
            }

            let raw = match note.freq {
                None => 0.0,
                Some(freq) => {
                    let is_first_sample = self.pos_in_note == 0;
                    let step = freq / SAMPLE_RATE.get() as f32;
                    self.phase += step;
                    let wrapped = self.phase >= 1.0;
                    if wrapped {
                        self.phase -= 1.0;
                    }
                    match note.waveform {
                        Waveform::Square => square_wave(self.phase),
                        Waveform::Triangle => triangle_wave(self.phase),
                        Waveform::Noise => {
                            if wrapped || is_first_sample {
                                self.noise_hold = lfsr_bit(&mut self.noise_state);
                            }
                            self.noise_hold
                        }
                    }
                }
            };

            let env = envelope(self.pos_in_note, note_len);
            self.pos_in_note += 1;
            return Some(raw * note.volume * env);
        }
    }
}

impl Source for Sequencer {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        nz!(1)
    }

    fn sample_rate(&self) -> SampleRate {
        SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.total_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_matches_known_pitches() {
        assert!((hz(A, 4) - 440.0).abs() < 1e-3);
        assert!((hz(A, 5) - 880.0).abs() < 1e-3);
        assert!((hz(A, 3) - 220.0).abs() < 1e-3);
        // C4 (middle C) は約261.63Hz。
        assert!((hz(C, 4) - 261.626).abs() < 1e-2);
    }

    #[test]
    fn sequencer_of_single_tone_ends_after_expected_sample_count() {
        let note = Note::tone(hz(A, 4), 0.01, Waveform::Square, 1.0);
        let expected_len = ((0.01 * SAMPLE_RATE.get() as f32).round() as usize).max(1);

        let mut seq = Sequencer::new(vec![note]);
        let samples: Vec<f32> = std::iter::from_fn(|| seq.next()).collect();

        assert_eq!(samples.len(), expected_len);
        // 全サンプルが音量1.0の矩形波の範囲(-1.0..=1.0)に収まっていること。
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0 + 1e-6));
    }

    #[test]
    fn rest_note_is_silent() {
        let mut seq = Sequencer::new(vec![Note::rest(0.005)]);
        let samples: Vec<f32> = std::iter::from_fn(|| seq.next()).collect();
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn empty_sequence_yields_no_samples() {
        let mut seq = Sequencer::new(vec![]);
        assert_eq!(seq.next(), None);
    }

    #[test]
    fn multiple_notes_are_played_in_order_and_total_duration_matches() {
        let notes = vec![
            Note::tone(hz(C, 5), 0.01, Waveform::Square, 0.5),
            Note::rest(0.01),
            Note::tone(hz(G, 5), 0.01, Waveform::Triangle, 0.5),
        ];
        let expected_total: f32 = notes.iter().map(|_| 0.01).sum();

        let seq = Sequencer::new(notes);
        let total = seq.total_duration().expect("finite duration expected");
        assert!((total.as_secs_f32() - expected_total).abs() < 1e-4);

        let samples: Vec<f32> = {
            let mut seq = seq;
            std::iter::from_fn(move || seq.next()).collect()
        };
        let per_note = ((0.01 * SAMPLE_RATE.get() as f32).round() as usize).max(1);
        assert_eq!(samples.len(), per_note * 3);
        // 休符区間(2番目のノート)は無音のはず。
        assert!(samples[per_note..per_note * 2].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn noise_waveform_never_panics_and_stays_bounded() {
        let note = Note::tone(1000.0, 0.02, Waveform::Noise, 0.8);
        let mut seq = Sequencer::new(vec![note]);
        let samples: Vec<f32> = std::iter::from_fn(|| seq.next()).collect();
        assert!(!samples.is_empty());
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 0.8 + 1e-6));
    }

    #[test]
    fn triangle_wave_shape_is_bounded_and_continuous_at_endpoints() {
        assert!((triangle_wave(0.0) - (-1.0)).abs() < 1e-6);
        assert!((triangle_wave(0.25) - 0.0).abs() < 1e-6);
        assert!((triangle_wave(0.5) - 1.0).abs() < 1e-6);
        assert!((triangle_wave(0.75) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn square_wave_shape_is_bipolar() {
        assert_eq!(square_wave(0.0), 1.0);
        assert_eq!(square_wave(0.49), 1.0);
        assert_eq!(square_wave(0.5), -1.0);
        assert_eq!(square_wave(0.99), -1.0);
    }
}
