//! CPU renderer: RGBA canvas with source-over blending, sprite blitting,
//! rounded rects and text. Pixels are converted to the softbuffer u32 format
//! (0xAARRGGBB) at present time.

use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};

pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>, // RGBA
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, px: vec![0; (w as usize) * (h as usize) * 4] }
    }
    pub fn clear(&mut self, rgba: [u8; 4]) {
        for chunk in self.px.chunks_exact_mut(4) {
            chunk.copy_from_slice(&rgba);
        }
    }
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || c[3] == 0 {
            return;
        }
        let i = ((y as usize) * self.w as usize + x as usize) * 4;
        let a = c[3] as u16;
        if a == 255 {
            self.px[i..i + 4].copy_from_slice(&c);
            return;
        }
        let da = 255 - c[3] as u16;
        for k in 0..3 {
            self.px[i + k] = ((c[k] as u16 * a + self.px[i + k] as u16 * da) / 255) as u8;
        }
        self.px[i + 3] = (c[3] as u16 + self.px[i + 3] as u16 * da / 255).min(255) as u8;
    }
    /// Rounded rectangle: filled body + 2px border.
    pub fn round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, fill: [u8; 4], border: [u8; 4]) {
        for yy in y..y + h {
            for xx in x..x + w {
                let dx = (x + r - xx).max(xx - (x + w - 1 - r)).max(0);
                let dy = (y + r - yy).max(yy - (y + h - 1 - r)).max(0);
                if dx * dx + dy * dy > r * r {
                    continue;
                }
                let edge = xx == x || yy == y || xx == x + w - 1 || yy == y + h - 1
                    || xx == x + 1 || yy == y + 1 || xx == x + w - 2 || yy == y + h - 2;
                self.blend(xx, yy, if edge { border } else { fill });
            }
        }
    }
    /// Nearest-neighbour sprite blit with alpha.
    pub fn blit(&mut self, frame: &crate::assets::Frame, x: i32, y: i32, scale: f64) {
        let sw = frame.w as i32;
        let sh = frame.h as i32;
        let dw = (frame.w as f64 * scale) as i32;
        let dh = (frame.h as f64 * scale) as i32;
        for dy in 0..dh {
            let sy = (dy * sh) / dh.max(1);
            for dx in 0..dw {
                let sx = (dx * sw) / dw.max(1);
                let i = ((sy as usize) * frame.w as usize + sx as usize) * 4;
                let c = [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2], frame.rgba[i + 3]];
                self.blend(x + dx, y + dy, c);
            }
        }
    }
}

/// Word-wrap `text` into lines fitting `max_w` pixels.
pub fn wrap(font: &FontArc, text: &str, max_w: f32, scale: PxScale) -> Vec<String> {
    let sf = font.as_scaled(scale);
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0.0f32;
        for word in para.split_whitespace() {
            let ww: f32 = word.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum();
            let sp = if cur.is_empty() { 0.0 } else { sf.h_advance(sf.glyph_id(' ')) };
            if cur_w + sp + ww > max_w && !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                cur.push_str(word);
                cur_w = ww;
            } else {
                if sp > 0.0 { cur.push(' '); }
                cur.push_str(word);
                cur_w += sp + ww;
            }
        }
        lines.push(cur);
    }
    lines
}
pub fn line_height(font: &FontArc, scale: PxScale) -> f32 {
    let sf = font.as_scaled(scale);
    (sf.ascent() - sf.descent() + sf.line_gap()).max(scale.y * 1.15)
}

/// Draw one line of text with its baseline at (x, y).
pub fn draw_line(canvas: &mut Canvas, font: &FontArc, text: &str, x: f32, y: f32, scale: PxScale, color: [u8; 4]) {
    let sf = font.as_scaled(scale);
    let mut x = x;
    for c in text.chars() {
        let adv = sf.h_advance(sf.glyph_id(c));
        let g = sf.glyph_id(c).with_scale_and_position(scale, point(x, y));
        if let Some(outline) = font.outline_glyph(g) {
            let bounds = outline.px_bounds();
            let (ox, oy) = (bounds.min.x as i32, bounds.min.y as i32);
            outline.draw(|gx, gy, cov| {
                let a = (cov.clamp(0.0, 1.0) * 255.0) as u8;
                canvas.blend(ox + gx as i32, oy + gy as i32, [color[0], color[1], color[2], a]);
            });
        }
        x += adv;
    }
}
