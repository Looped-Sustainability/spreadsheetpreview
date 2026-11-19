use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use serde::Deserialize;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Png,
    #[serde(alias = "jpg")]
    Jpeg,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub format: Format,
}

pub fn first_page(data: Vec<u8>, format: Format) -> Result<Vec<u8>, &'static str> {
    let pdf = Pdf::new(data).map_err(|_| "invalid or password-protected PDF")?;
    let page = pdf.pages().iter().next().ok_or("PDF has no pages")?;
    let (width, height) = page.render_dimensions();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("invalid PDF page dimensions");
    }
    let scale = (1600.0 / width.max(height)).min(2.0);
    if width.min(height) * scale < 1.0 {
        return Err("invalid PDF page dimensions");
    }

    let page = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let (width, height) = (page.width(), page.height());
    let pixels = page.data_as_u8_slice();
    let mut output = Vec::new();
    match format {
        Format::Png => {
            let mut encoder = png::Encoder::new(&mut output, u32::from(width), u32::from(height));
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|_| "PNG encoding failed")?;
            writer
                .write_image_data(pixels)
                .map_err(|_| "PNG encoding failed")?;
            writer.finish().map_err(|_| "PNG encoding failed")?;
        }
        Format::Jpeg => {
            jpeg_encoder::Encoder::new(&mut output, 90)
                .encode(pixels, width, height, jpeg_encoder::ColorType::Rgba)
                .map_err(|_| "JPEG encoding failed")?;
        }
    }
    Ok(output)
}
