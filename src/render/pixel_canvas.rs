//! 論理ピクセルグリッド + ハーフブロック疑似2倍解像度変換。
//!
//! `aquaterm`(`src/framebuffer.rs`)の手法を踏襲する: 論理ピクセルを縦2つ1組にし、
//! 上半分ブロック文字 `▀`(U+2580, fg=上ピクセル色, bg=下ピクセル色)で1端末セルに
//! まとめて描画する。端末フォントは縦長なため、この方式で見た目がほぼ正方形の
//! ピクセルになる。`scale` はズーム倍率で、1論理ピクセルを `scale × scale` 個の
//! 疑似ピクセルとして最近傍拡大してから変換する。

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::sprites::Sprite;

/// 論理ピクセルグリッド。原点は左上、(x, y) は (列, 行)。
pub struct PixelCanvas {
    width: usize,
    height: usize,
    pixels: Vec<Color>,
}

impl PixelCanvas {
    pub fn new(width: usize, height: usize, background: Color) -> Self {
        PixelCanvas {
            width,
            height,
            pixels: vec![background; width * height],
        }
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(y * self.width + x)
        } else {
            None
        }
    }

    /// 範囲内であれば指定座標を塗る。範囲外は無視する(呼び出し側での境界チェックを省ける)。
    pub fn set(&mut self, x: usize, y: usize, color: Color) {
        if let Some(idx) = self.index(x, y) {
            self.pixels[idx] = color;
        }
    }

    fn get(&self, x: usize, y: usize) -> Color {
        match self.index(x, y) {
            Some(idx) => self.pixels[idx],
            None => Color::Reset,
        }
    }

    /// `(origin_x, origin_y)` を左上として `sprite` を描き込む。
    /// スプライトの透過ピクセル(`None`)は下地の色をそのまま残す。
    pub fn blit_sprite(&mut self, origin_x: usize, origin_y: usize, sprite: &Sprite) {
        for (sy, row) in sprite.pixels.iter().enumerate() {
            for (sx, pixel) in row.iter().enumerate() {
                if let Some(color) = pixel {
                    self.set(origin_x + sx, origin_y + sy, *color);
                }
            }
        }
    }

    /// `scale` 倍(1論理ピクセル→scale×scale)に最近傍拡大しつつ、
    /// ハーフブロック文字でratatuiの `Line` 列に変換する。
    pub fn to_lines(&self, scale: usize) -> Vec<Line<'static>> {
        let scale = scale.max(1);
        let scaled_width = self.width * scale;
        let scaled_height = self.height * scale;

        let mut lines = Vec::with_capacity(scaled_height.div_ceil(2));
        let mut row = 0;
        while row < scaled_height {
            let top_logical_y = row / scale;
            let bottom_logical_y = (row + 1) / scale;

            let mut spans = Vec::with_capacity(scaled_width);
            for col in 0..scaled_width {
                let logical_x = col / scale;
                let top = self.get(logical_x, top_logical_y);
                let bottom = if row + 1 < scaled_height {
                    self.get(logical_x, bottom_logical_y)
                } else {
                    // 高さが奇数の場合、最終行は上ピクセルのみを使い下段は同色で埋める。
                    top
                };
                spans.push(Span::styled("▀", Style::default().fg(top).bg(bottom)));
            }
            lines.push(Line::from(spans));
            row += 2;
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_fills_with_background_color() {
        let canvas = PixelCanvas::new(4, 4, Color::Rgb(1, 2, 3));
        assert_eq!(canvas.get(0, 0), Color::Rgb(1, 2, 3));
        assert_eq!(canvas.get(3, 3), Color::Rgb(1, 2, 3));
    }

    #[test]
    fn set_out_of_bounds_is_ignored_not_panicking() {
        let mut canvas = PixelCanvas::new(2, 2, Color::Black);
        canvas.set(100, 100, Color::White);
        assert_eq!(canvas.get(0, 0), Color::Black);
    }

    #[test]
    fn to_lines_pairs_rows_into_half_block_lines() {
        // 高さ2の単色キャンバスは1行のハーフブロック行にまとまるはず。
        let canvas = PixelCanvas::new(2, 2, Color::Rgb(9, 9, 9));
        let lines = canvas.to_lines(1);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn to_lines_handles_odd_height_by_reusing_top_pixel_for_the_last_row() {
        let canvas = PixelCanvas::new(1, 3, Color::Rgb(5, 5, 5));
        let lines = canvas.to_lines(1);
        // height=3 -> ceil(3/2) = 2 行。
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn to_lines_scale_multiplies_output_dimensions() {
        let canvas = PixelCanvas::new(2, 2, Color::Rgb(1, 1, 1));
        let lines = canvas.to_lines(3);
        // 幅方向: (2*3) = 6 スパン、高さ方向: ceil(2*3/2) = 3 行。
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].spans.len(), 6);
    }
}
