//! PNG input: Sirius's `crop_384` and `convert_to_1bit`, minus the dithering.
//!
//! Sirius's `convert_to_1bit` is `im.convert('1')`, which Pillow dithers
//! Floyd-Steinberg. Reproducing Pillow's dither is out of scope, so we require
//! an input that is already pure black and white, where `convert('1')` is a
//! no-op. Anything else is rejected rather than silently encoded differently
//! from Sirius.

use std::path::Path;

use crate::encode::{MAX_HEIGHT, WIDTH};

/// Decode a PNG into a row-major bitmap, true = white, cropped to 384 px wide.
///
/// Returns (pixels, width, height).
pub fn load_bw(path: &Path) -> Result<(Vec<bool>, u32, u32), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // EXPAND lifts palette and sub-8-bit grey to 8 bits; STRIP_16 drops 16-bit
    // samples to 8. Together they leave 1 to 4 samples of 8 bits per pixel.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;

    if info.width < WIDTH {
        // Pillow's crop would pad the shortfall with black, which inverts the
        // print silently. Refuse instead.
        return Err(format!(
            "{}: {} px wide, needs at least {WIDTH}",
            path.display(),
            info.width
        ));
    }
    let height = info.height.min(MAX_HEIGHT); // crop_384
    let samples = info.color_type.samples();
    // Alpha is dropped, matching Pillow's convert('1') from LA or RGBA.
    let colour = match info.color_type {
        png::ColorType::GrayscaleAlpha | png::ColorType::Rgba => samples - 1,
        _ => samples,
    };

    let mut out = Vec::with_capacity((WIDTH * height) as usize);
    for y in 0..height as usize {
        let row = &buf[y * info.line_size..][..info.line_size];
        for x in 0..WIDTH as usize {
            let px = &row[x * samples..][..colour];
            let white = match (px.iter().all(|&v| v == 255), px.iter().all(|&v| v == 0)) {
                (true, _) => true,
                (_, true) => false,
                _ => {
                    return Err(format!(
                        "{}: pixel ({x}, {y}) is {px:?}, not pure black or white. \
                         Convert to 1-bit first; this encoder does not dither.",
                        path.display()
                    ))
                }
            };
            out.push(white);
        }
    }
    Ok((out, WIDTH, height))
}
