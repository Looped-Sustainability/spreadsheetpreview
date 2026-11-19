use std::collections::HashMap;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

use crate::table::Table;

const INK: [u8; 3] = [30, 47, 53];
const MUTED: [u8; 3] = [77, 100, 103];
const LINE: [u8; 3] = [212, 223, 222];
const STRIPE: [u8; 3] = [246, 249, 248];
const WHITE: [u8; 3] = [255; 3];
const FONT: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

#[derive(Clone, Copy)]
struct TextStyle {
    size: u8,
    color: [u8; 3],
}

const TITLE: TextStyle = TextStyle {
    size: 21,
    color: WHITE,
};
const CAPTION: TextStyle = TextStyle {
    size: 14,
    color: MUTED,
};
const CELL: TextStyle = TextStyle {
    size: 17,
    color: INK,
};

struct Glyph {
    advance: usize,
    left: i32,
    top: i32,
    width: usize,
    alpha: Vec<u8>,
}

struct Canvas {
    width: usize,
    height: usize,
    rgb: Vec<u8>,
    font: FontRef<'static>,
    glyphs: HashMap<(char, u8), Glyph>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            rgb: vec![255; width * height * 3],
            font: FontRef::try_from_slice(FONT).expect("bundled font"),
            glyphs: HashMap::new(),
        }
    }

    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: [u8; 3]) {
        for row in y..y + h {
            let start = 3 * (row * self.width + x);
            for pixel in self.rgb[start..start + 3 * w].chunks_exact_mut(3) {
                pixel.copy_from_slice(&color);
            }
        }
    }

    fn glyph(&mut self, ch: char, size: u8) -> &Glyph {
        self.glyphs.entry((ch, size)).or_insert_with(|| {
            let scale = PxScale::from(size as f32);
            let id = self.font.glyph_id(ch);
            let advance = self.font.as_scaled(scale).h_advance(id).round() as usize;
            let mut raster = Glyph {
                advance,
                left: 0,
                top: 0,
                width: 0,
                alpha: Vec::new(),
            };
            if let Some(outline) = self
                .font
                .outline_glyph(id.with_scale_and_position(scale, point(0.0, 0.0)))
            {
                let bounds = outline.px_bounds();
                raster.width = bounds.width() as usize;
                raster.left = bounds.min.x as i32;
                raster.top = bounds.min.y as i32;
                raster
                    .alpha
                    .resize(raster.width * bounds.height() as usize, 0);
                outline.draw(|x, y, coverage| {
                    raster.alpha[y as usize * raster.width + x as usize] =
                        (coverage * 255.0).round() as u8;
                });
            }
            raster
        })
    }

    fn measure(&mut self, s: &str, size: u8) -> usize {
        s.chars().map(|c| self.glyph(c, size).advance).sum()
    }

    fn label(
        &mut self,
        s: &str,
        x: usize,
        baseline: usize,
        max: usize,
        right: bool,
        style: TextStyle,
    ) {
        let TextStyle { size, color } = style;
        let width = self.measure(s, size);
        let clipped;
        let s = if width > max {
            let mut used = self.glyph('…', size).advance;
            let mut out = String::new();
            for c in s.chars() {
                used += self.glyph(c, size).advance;
                if used > max {
                    break;
                }
                out.push(c);
            }
            out.push('…');
            clipped = out;
            &clipped
        } else {
            s
        };
        let mut pen = x + if right {
            max.saturating_sub(self.measure(s, size))
        } else {
            0
        };
        for c in s.chars() {
            let glyph = &self.glyphs[&(c, size)];
            for (index, &a) in glyph.alpha.iter().enumerate() {
                let gx = pen as i32 + glyph.left + (index % glyph.width) as i32;
                let gy = baseline as i32 + glyph.top + (index / glyph.width) as i32;
                if gx < x as i32 || gx >= (x + max) as i32 || gy < 0 || gy >= self.height as i32 {
                    continue;
                }
                let offset = 3 * (gy as usize * self.width + gx as usize);
                for (pixel, ink) in self.rgb[offset..offset + 3].iter_mut().zip(color) {
                    *pixel = ((u32::from(*pixel) * (255 - u32::from(a))
                        + u32::from(ink) * u32::from(a))
                        / 255) as u8;
                }
            }
            pen += glyph.advance;
        }
    }
}

pub fn png(table: &Table) -> Result<Vec<u8>, &'static str> {
    let mut canvas = Canvas::new(1, 1);
    let mut widths = vec![78; table.width.max(1)];
    for row in &table.rows {
        for (i, value) in row.iter().enumerate() {
            widths[i] = widths[i].max((canvas.measure(&value.text, CELL.size) + 24).min(208));
        }
    }
    let gutter = 42;
    let width = (widths.iter().sum::<usize>() + gutter + 1).max(270);
    let extra = width - gutter - 1 - widths.iter().sum::<usize>();
    *widths.last_mut().unwrap() += extra;
    let bottom = 70 + table.rows.len().max(1) * 30;
    let height = bottom + 32;
    canvas.width = width;
    canvas.height = height;
    canvas.rgb.resize(width * height * 3, 255);
    canvas.rect(0, 0, width, 44, [20, 64, 66]);
    canvas.rect(15, 15, 16, 14, [173, 215, 202]);
    canvas.rect(16, 19, 14, 1, [20, 64, 66]);
    canvas.rect(21, 19, 1, 9, [20, 64, 66]);
    canvas.label(&table.title, 42, 29, width - 56, false, TITLE);
    canvas.rect(0, 44, width, 26, [230, 239, 236]);
    canvas.rect(0, 70, gutter, bottom - 70, STRIPE);
    let mut x = gutter;
    for (i, &w) in widths.iter().enumerate() {
        let letter = char::from(b'A' + i as u8).to_string();
        canvas.label(&letter, x + 12, 62, w - 24, false, CAPTION);
        x += w;
    }
    for (i, row) in table.rows.iter().enumerate() {
        let y = 70 + i * 30;
        if i % 2 == 1 {
            canvas.rect(gutter, y, width - gutter, 30, [250, 252, 251]);
        }
        canvas.label(&(i + 1).to_string(), 4, y + 20, gutter - 14, true, CAPTION);
        let mut x = gutter;
        for (cell, &w) in row.iter().zip(&widths) {
            canvas.label(&cell.text, x + 12, y + 21, w - 24, cell.numeric, CELL);
            x += w;
        }
        canvas.rect(0, y, width, 1, LINE);
    }
    let mut x = gutter;
    for &w in &widths {
        canvas.rect(x, 44, 1, bottom - 44, LINE);
        x += w;
    }
    canvas.rect(0, bottom, width, 1, LINE);
    let footer = if table.cropped {
        "Partial sheet"
    } else if table.rows.is_empty() {
        "Empty worksheet"
    } else {
        "Sheet preview"
    };
    canvas.label(footer, 13, bottom + 21, width - 26, false, CAPTION);
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|_| "PNG encoding failed")?;
        writer
            .write_image_data(&canvas.rgb)
            .map_err(|_| "PNG encoding failed")?;
        writer.finish().map_err(|_| "PNG encoding failed")?;
    }
    Ok(output)
}
