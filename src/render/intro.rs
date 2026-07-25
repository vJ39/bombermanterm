//! 起動時のオンボーディング画面。
//!
//! `termmap`(`src/main.rs` の `--image` 経路: `image::open` → リサイズ →
//! ハーフブロック変換)の手法を踏襲する: 静止画像を読み込み、端末表示用の
//! 解像度にリサイズしてから [`PixelCanvas`] へ展開し、起動直後に一度表示して
//! 何らかのキー入力で閉じる。画像はビルド時に `include_bytes!` でバイナリへ
//! 埋め込むため、実行時に外部ファイルへは依存しない。

use image::imageops::FilterType;
use image::{GenericImageView, Rgb};
use ratatui::style::Color;

use super::pixel_canvas::PixelCanvas;

/// 埋め込み画像(`assets/intro.png`)。ユーザー提供のオリジナルデザイン
/// (本家キャラクターの意匠は含まない、ロボット風マスコット)。
const INTRO_IMAGE_BYTES: &[u8] = include_bytes!("../../assets/intro.png");

/// 表示解像度(論理ピクセル)。元画像は正方形なので幅=高さにする。
const DISPLAY_SIZE: u32 = 96;

/// ほぼ白の画素は元画像の背景なので、キャラクターと分離して透過扱いにする
/// (このしきい値以上ならすべて背景とみなす)。
const WHITE_THRESHOLD: u8 = 235;

/// オンボーディング画面用のピクセルキャンバスを組み立てる。
/// 呼び出しごとに画像のデコード・リサイズを行う(起動時に1回しか呼ばないため
/// キャッシュは設けていない)。
pub fn build_canvas() -> PixelCanvas {
    let background = Color::Rgb(10, 14, 24);

    let decoded = image::load_from_memory(INTRO_IMAGE_BYTES)
        .expect("assets/intro.png must be a valid, bundled PNG");
    let (src_w, src_h) = decoded.dimensions();
    let (target_w, target_h) = if src_w >= src_h {
        (DISPLAY_SIZE, DISPLAY_SIZE * src_h / src_w.max(1))
    } else {
        (DISPLAY_SIZE * src_w / src_h.max(1), DISPLAY_SIZE)
    };
    let resized = decoded
        .resize_exact(target_w.max(1), target_h.max(1), FilterType::Triangle)
        .to_rgb8();

    let mut canvas = PixelCanvas::new(resized.width() as usize, resized.height() as usize, background);
    for (x, y, Rgb([r, g, b])) in resized.enumerate_pixels() {
        if *r >= WHITE_THRESHOLD && *g >= WHITE_THRESHOLD && *b >= WHITE_THRESHOLD {
            continue; // 背景(白地)は透過のままにする。
        }
        canvas.set(x as usize, y as usize, Color::Rgb(*r, *g, *b));
    }

    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_canvas_produces_non_empty_output() {
        let canvas = build_canvas();
        let lines = canvas.to_lines(1);
        assert!(!lines.is_empty());
    }
}
