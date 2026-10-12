//! RustImageBackend internals (spec §16.1, D19): pure-`image` decode → resize
//! (triangle filter) → encode PNG/JPEG/WebP, plus LQIP/blur placeholders.
//!
//! Documented scope decisions (Task 9):
//! - WebP uses the `image` crate's **lossless-only** encoder. If a future/alternate
//!   build rejects an encode, we fall back to PNG, set `result.format` to the actual
//!   output format (PNG) and append a warning diagnostic with code `AST-WEBP`.
//! - AVIF encode is feature-gated: `?format=avif` returns `AST-AVIF` pointing at the
//!   libvips `ImageBackend` (ravif/oxipng land with the lightningcss/dav1d toolchain).
//! - Placeholder pipeline: resize to 16px wide (aspect preserved), JPEG; `Blur` adds
//!   two extra blur passes (`imageops::blur`, σ=2) — the classic LQIP look.
//! - `fast_image_resize`, `ravif`, `webp`, `oxipng` per §16.1 are backend-swap items,
//!   same as libvips (D19); the default backend stays dependency-minimal.

use crate::{
    err, Diagnostic, ImageBackend, ImageFormat, ImageRequest, ImageResult, PlaceholderKind, Result,
};
use blake3::Hasher;
use image::imageops::FilterType;
use image::ImageEncoder as _;
use std::io::Cursor;

/// Placeholder width in px (§16.1 LQIP convention).
const PLACEHOLDER_WIDTH: u32 = 16;

impl ImageBackend for crate::RustImageBackend {
    fn process(&self, req: ImageRequest) -> Result<ImageResult> {
        let img = image::load_from_memory(&req.source)
            .map_err(|e| err("AST-IMG", format!("failed to decode source image: {e}")))?;
        let (ow, oh) = (img.width(), img.height());
        let (tw, th) = target_dims(ow, oh, req.width, req.height);

        let rgba = if (tw, th) != (ow, oh) {
            image::imageops::resize(&img.to_rgba8(), tw, th, FilterType::Triangle)
        } else {
            img.to_rgba8()
        };
        let (fw, fh) = (rgba.width(), rgba.height());
        let placeholder = req.placeholder.map(|kind| make_placeholder(&rgba, kind));

        let (bytes, format, diagnostics) = encode(&rgba, req.format, req.quality)?;
        let cache_key = cache_key(&req, fw, fh);

        Ok(ImageResult {
            bytes,
            format,
            width: fw,
            height: fh,
            placeholder,
            cache_key,
            diagnostics,
        })
    }
}

/// Width wins when both are set (aspect preserved, §16.1 "intrinsic dimensions").
fn target_dims(ow: u32, oh: u32, width: Option<u32>, height: Option<u32>) -> (u32, u32) {
    let (w, h) = match (width, height) {
        (Some(w), _) => (w, ((oh as f64) * w as f64 / ow as f64).round() as u32),
        (None, Some(h)) => (((ow as f64) * h as f64 / oh as f64).round() as u32, h),
        (None, None) => (ow, oh),
    };
    (w.max(1), h.max(1))
}

fn encode(
    rgba: &image::RgbaImage,
    format: ImageFormat,
    quality: u8,
) -> Result<(Vec<u8>, ImageFormat, Vec<Diagnostic>)> {
    let (w, h) = (rgba.width(), rgba.height());
    match format {
        ImageFormat::Png => {
            let mut cursor = Cursor::new(Vec::new());
            image::codecs::png::PngEncoder::new(&mut cursor)
                .write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
                .map_err(|e| err("AST-IMG", format!("png encode failed: {e}")))?;
            Ok((cursor.into_inner(), ImageFormat::Png, Vec::new()))
        }
        ImageFormat::Jpeg => {
            let q = quality.clamp(1, 100);
            let rgb = image::DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
            let mut cursor = Cursor::new(Vec::new());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, q)
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| err("AST-IMG", format!("jpeg encode failed: {e}")))?;
            Ok((cursor.into_inner(), ImageFormat::Jpeg, Vec::new()))
        }
        ImageFormat::Webp => {
            let mut cursor = Cursor::new(Vec::new());
            let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut cursor);
            match enc.encode(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8) {
                Ok(()) => Ok((cursor.into_inner(), ImageFormat::Webp, Vec::new())),
                Err(e) => {
                    // Documented fallback: report the requested-format failure with
                    // `AST-WEBP`, return PNG bytes, `format` = actual output (Png).
                    let png = encode_png_fallback(rgba)?;
                    let diag = Diagnostic::warning(
                        "AST-WEBP",
                        format!(
                            "webp encode unavailable in this image build; emitted PNG instead: {e}"
                        ),
                    );
                    Ok((png, ImageFormat::Png, vec![diag]))
                }
            }
        }
        ImageFormat::Avif => {
            let bytes = encode_avif(rgba, quality)?;
            Ok((bytes, ImageFormat::Avif, Vec::new()))
        }
    }
}

/// AVIF encode via `ravif` (spec §16.1: real AVIF output with the pure-Rust stack).
/// The `image` crate gives us RGBA8; ravif wants `Img<&[RGBA8]>`.
fn encode_avif(rgba: &image::RgbaImage, quality: u8) -> Result<Vec<u8>> {
    use ravif::{Encoder, Img, RGBA8};
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let pixels: Vec<RGBA8> = rgba
        .pixels()
        .map(|p| RGBA8::new(p[0], p[1], p[2], p[3]))
        .collect();
    let img = Img::new(pixels.as_slice(), w, h);
    let q = (quality.clamp(1, 100)) as f32;
    let encoded = Encoder::new()
        .with_quality(q)
        .with_speed(6)
        .encode_rgba(img)
        .map_err(|e| err("AST-AVIF", format!("ravif encode failed: {e}")))?;
    Ok(encoded.avif_file)
}

fn encode_png_fallback(rgba: &image::RgbaImage) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    image::codecs::png::PngEncoder::new(&mut cursor)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| err("AST-IMG", format!("png fallback encode failed: {e}")))?;
    Ok(cursor.into_inner())
}

fn make_placeholder(rgba: &image::RgbaImage, kind: PlaceholderKind) -> String {
    let (ow, oh) = (rgba.width(), rgba.height());
    let w = PLACEHOLDER_WIDTH.min(ow).max(1);
    let h = (((oh as f64) * w as f64 / ow as f64).round() as u32).max(1);
    let mut small = image::imageops::resize(rgba, w, h, FilterType::Triangle);
    if kind == PlaceholderKind::Blur {
        // Two blur passes (σ=2) ≈ the classic box-blur LQIP smear.
        small = image::imageops::blur(&small, 2.0);
        small = image::imageops::blur(&small, 2.0);
    }
    let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
    let mut cursor = Cursor::new(Vec::new());
    // Tiny image; encode cannot realistically fail, but fall back to empty payload
    // rather than panicking.
    let ok = image::codecs::jpeg::JpegEncoder::new(&mut cursor)
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .is_ok();
    if !ok {
        return String::new();
    }
    format!(
        "data:image/jpeg;base64,{}",
        base64_encode(&cursor.into_inner())
    )
}

fn cache_key(req: &ImageRequest, out_w: u32, out_h: u32) -> String {
    let mut h = Hasher::new();
    h.update(&req.source);
    h.update(req.source_format.mime().as_bytes());
    h.update(&req.width.unwrap_or(0).to_le_bytes());
    h.update(&req.height.unwrap_or(0).to_le_bytes());
    h.update(req.format.mime().as_bytes());
    h.update(&[
        req.quality,
        match req.placeholder {
            None => 0u8,
            Some(PlaceholderKind::Lqip) => 1,
            Some(PlaceholderKind::Blur) => 2,
        },
    ]);
    h.update(&out_w.to_le_bytes());
    h.update(&out_h.to_le_bytes());
    let hex = h.finalize().to_hex().to_string();
    hex[..16].to_string()
}

/// Minimal standard base64 (RFC 4648, padded) — avoids adding a base64 dependency.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
