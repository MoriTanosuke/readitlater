//! Bilder aus Artikeln für das Lesen verkleinern.
//!
//! Die Bytes stammen von fremden Servern und gelten als feindlich. Darum:
//! - Das Format wird an den Bytes erkannt (nie am Content-Type), erlaubt sind JPEG, PNG, GIF
//!   und WebP. Kein SVG, kein AVIF.
//! - Die Maße stehen im Kopf der Datei und werden **vor** dem Dekodieren geprüft
//!   (Bildbombe: kleine Datei, riesige Pixelzahl). Zusätzlich begrenzt `Limits` den Speicher.
//! - Dekodiert wird nur, was auch verkleinert oder neu kodiert wird. Beim Neukodieren fallen
//!   EXIF-Daten (etwa der Aufnahmeort) weg; die Drehung aus EXIF wird vorher angewendet.
//!
//! Regeln: JPEG wird immer neu kodiert (Qualität 82), PNG bleibt PNG (große PNG ohne
//! Transparenz werden als JPEG kodiert, wenn das deutlich kleiner ist), WebP wird nur bei
//! mehr als 1600 px neu kodiert (die Bibliothek schreibt WebP nur verlustfrei), GIF bleibt
//! unverändert (Animation). Längste Seite höchstens 1600 px.

use std::io::Cursor;

use image::{
    DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType as PngFilter, PngEncoder},
    },
    imageops::FilterType,
};

/// Längste Seite nach dem Verkleinern.
pub const MAX_DIMENSION: u32 = 1600;
/// Mehr Pixel werden gar nicht erst dekodiert (40 Megapixel).
pub const MAX_PIXELS: u64 = 40_000_000;
/// Mehr als das wird nicht gespeichert (auch nicht bei unverändert übernommenen Bildern).
pub const MAX_STORED_BYTES: usize = 5 * 1024 * 1024;
/// Bilder, die nur ein Zählpixel oder Platzhalter sein können, sind keinen Speicher wert.
const MIN_DIMENSION: u32 = 8;
const JPEG_QUALITY: u8 = 82;
/// Ab dieser PNG-Größe (ohne Transparenz) lohnt der Versuch mit JPEG.
const PNG_TO_JPEG_FROM: usize = 512 * 1024;
/// Speicherlimit der Bibliothek beim Dekodieren (40 MP RGBA sind 160 MB).
const MAX_ALLOC: u64 = 400 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ProcessError {
    /// Kein JPEG, PNG, GIF oder WebP.
    Unsupported,
    /// Zu viele Pixel oder Ergebnis zu groß.
    TooBig,
    /// Winziges Bild (Zählpixel, Platzhalter).
    Tiny,
    /// Defekte oder nicht lesbare Datei.
    Invalid,
}

#[derive(Debug)]
pub struct Processed {
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

fn mime_of(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => "image/jpeg",
    }
}

/// Prüft und verkleinert ein heruntergeladenes Bild. Rechenintensiv: nicht im
/// asynchronen Kontext aufrufen, sondern in `spawn_blocking`.
pub fn process(bytes: &[u8]) -> Result<Processed, ProcessError> {
    let format = image::guess_format(bytes).map_err(|_| ProcessError::Unsupported)?;
    if !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return Err(ProcessError::Unsupported);
    }

    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits());
    let mut decoder = reader.into_decoder().map_err(|_| ProcessError::Invalid)?;
    let (width, height) = decoder.dimensions();
    if width < MIN_DIMENSION && height < MIN_DIMENSION {
        return Err(ProcessError::Tiny);
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ProcessError::TooBig);
    }
    let fits = width.max(height) <= MAX_DIMENSION;

    // Ohne Neukodieren übernommen: GIF (Animation) und WebP in passender Größe.
    if format == ImageFormat::Gif || (format == ImageFormat::WebP && fits) {
        if bytes.len() > MAX_STORED_BYTES {
            return Err(ProcessError::TooBig);
        }
        return Ok(Processed {
            mime: mime_of(format),
            width,
            height,
            data: bytes.to_vec(),
        });
    }

    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(|_| ProcessError::Invalid)?;
    img.apply_orientation(orientation);
    if img.width().max(img.height()) > MAX_DIMENSION {
        img = img.resize(MAX_DIMENSION, MAX_DIMENSION, FilterType::Lanczos3);
    }

    let has_alpha = img.color().has_alpha();
    let (mime, data) = match format {
        ImageFormat::Jpeg => ("image/jpeg", encode_jpeg(&img)?),
        // Verlustfreies WebP kodieren wir nicht neu; wer zu groß war, wird JPEG oder PNG.
        _ if has_alpha => ("image/png", encode_png(&img)?),
        _ => {
            let png = encode_png(&img)?;
            if format == ImageFormat::Png && png.len() <= PNG_TO_JPEG_FROM {
                ("image/png", png)
            } else {
                let jpeg = encode_jpeg(&img)?;
                // Bei PNG nur wechseln, wenn es deutlich spart (Text und Grafiken leiden).
                if format == ImageFormat::Png && jpeg.len() * 10 > png.len() * 6 {
                    ("image/png", png)
                } else {
                    ("image/jpeg", jpeg)
                }
            }
        }
    };
    if data.len() > MAX_STORED_BYTES {
        return Err(ProcessError::TooBig);
    }
    Ok(Processed {
        mime,
        width: img.width(),
        height: img.height(),
        data,
    })
}

fn encode_jpeg(img: &DynamicImage) -> Result<Vec<u8>, ProcessError> {
    // JPEG kennt keine Transparenz: auf RGB umrechnen.
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|_| ProcessError::Invalid)?;
    Ok(out)
}

fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, ProcessError> {
    let mut out = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, PngFilter::Adaptive);
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        encoder.write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
    } else {
        let rgb = img.to_rgb8();
        encoder.write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
    }
    .map_err(|_| ProcessError::Invalid)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    /// Weiche Muster mit leichtem Rauschen: für PNG kaum komprimierbar, für JPEG schon (wie ein Foto).
    fn photo(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, y| {
            let (fx, fy) = (f64::from(x), f64::from(y));
            let hash = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503).rotate_left(13);
            let noise = f64::from((hash >> 7) & 7) - 3.5;
            let channel = |phase: f64| {
                let v = 128.0 + 90.0 * ((fx / 37.0 + phase).sin() * (fy / 53.0 - phase).cos());
                (v + noise).clamp(0.0, 255.0) as u8
            };
            Rgb([channel(0.0), channel(2.0), channel(4.0)])
        })
    }

    fn jpeg_bytes(img: &RgbImage) -> Vec<u8> {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 95)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        out
    }

    fn png_bytes(img: &DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn large_jpeg_is_scaled_to_max_dimension_and_smaller() {
        let original = jpeg_bytes(&photo(2000, 1500));
        let done = process(&original).unwrap();
        assert_eq!(done.mime, "image/jpeg");
        assert_eq!((done.width, done.height), (1600, 1200));
        assert!(done.data.len() < original.len());
        assert_eq!(image::guess_format(&done.data).unwrap(), ImageFormat::Jpeg);
        let decoded = image::load_from_memory(&done.data).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1600, 1200));
    }

    #[test]
    fn small_jpeg_keeps_size_but_is_reencoded() {
        let original = jpeg_bytes(&photo(400, 300));
        let done = process(&original).unwrap();
        assert_eq!((done.width, done.height), (400, 300));
        assert_eq!(done.mime, "image/jpeg");
    }

    #[test]
    fn portrait_is_limited_by_height() {
        let done = process(&jpeg_bytes(&photo(1000, 3000))).unwrap();
        assert_eq!(done.height, 1600);
        assert_eq!(done.width, 533);
    }

    #[test]
    fn png_with_transparency_stays_png() {
        let mut img = RgbaImage::from_pixel(2000, 1000, Rgba([10, 20, 30, 0]));
        img.put_pixel(5, 5, Rgba([255, 0, 0, 255]));
        let done = process(&png_bytes(&DynamicImage::ImageRgba8(img))).unwrap();
        assert_eq!(done.mime, "image/png");
        assert_eq!((done.width, done.height), (1600, 800));
        assert!(
            image::load_from_memory(&done.data)
                .unwrap()
                .color()
                .has_alpha()
        );
    }

    #[test]
    fn large_opaque_png_photo_becomes_jpeg() {
        let original = png_bytes(&DynamicImage::ImageRgb8(photo(1500, 1200)));
        assert!(original.len() > PNG_TO_JPEG_FROM);
        let done = process(&original).unwrap();
        assert_eq!(done.mime, "image/jpeg");
        assert!(done.data.len() < original.len() / 2);
    }

    #[test]
    fn flat_graphic_stays_png() {
        let img = RgbImage::from_pixel(1200, 800, Rgb([240, 240, 240]));
        let done = process(&png_bytes(&DynamicImage::ImageRgb8(img))).unwrap();
        assert_eq!(done.mime, "image/png");
    }

    #[test]
    fn gif_is_kept_unchanged() {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([1, 2, 3, 255])))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Gif)
            .unwrap();
        let done = process(&bytes).unwrap();
        assert_eq!(done.mime, "image/gif");
        assert_eq!(done.data, bytes);
        assert_eq!((done.width, done.height), (64, 64));
    }

    #[test]
    fn small_lossless_webp_is_kept_unchanged() {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(photo(200, 100))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::WebP)
            .unwrap();
        let done = process(&bytes).unwrap();
        assert_eq!(done.mime, "image/webp");
        assert_eq!(done.data, bytes);
    }

    #[test]
    fn large_webp_is_scaled_and_recoded() {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(photo(2000, 1000))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::WebP)
            .unwrap();
        let done = process(&bytes).unwrap();
        assert_eq!((done.width, done.height), (1600, 800));
        assert!(done.mime == "image/jpeg" || done.mime == "image/png");
    }

    /// EXIF-Drehung (Orientation 6 = 90° im Uhrzeigersinn) wird angewendet: Breite und Höhe
    /// tauschen, und der Aufnahmeort fällt weg.
    #[test]
    fn exif_orientation_is_applied_and_exif_removed() {
        let plain = jpeg_bytes(&photo(300, 200));
        // APP1-Segment mit Exif (Orientation = 6, Big Endian) hinter SOI einfügen.
        let mut exif: Vec<u8> = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
        exif.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
        let mut with_exif = vec![0xFF, 0xD8, 0xFF, 0xE1];
        with_exif.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        with_exif.extend_from_slice(&exif);
        with_exif.extend_from_slice(&plain[2..]);

        let done = process(&with_exif).unwrap();
        assert_eq!((done.width, done.height), (200, 300), "gedreht");
        assert!(!done.data.windows(4).any(|w| w == b"Exif"), "EXIF entfernt");
    }

    #[test]
    fn too_many_pixels_are_rejected_before_decoding() {
        // Gültiger Kopf eines 8000x8000-PNG (64 MP) ohne Bilddaten: darf nicht dekodiert werden.
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(
                &vec![0u8; 8000 * 8000],
                8000,
                8000,
                image::ExtendedColorType::L8,
            )
            .unwrap();
        assert_eq!(process(&bytes).unwrap_err(), ProcessError::TooBig);
    }

    #[test]
    fn rejects_other_formats_and_garbage() {
        assert_eq!(
            process(b"<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap_err(),
            ProcessError::Unsupported
        );
        assert_eq!(process(b"").unwrap_err(), ProcessError::Unsupported);
        // Gültige Kennung, aber kein lesbares Bild.
        let mut broken = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        broken.extend_from_slice(&[7u8; 64]);
        assert_eq!(process(&broken).unwrap_err(), ProcessError::Invalid);
    }

    #[test]
    fn tiny_images_are_rejected() {
        let tiny = png_bytes(&DynamicImage::ImageRgb8(RgbImage::new(1, 1)));
        assert_eq!(process(&tiny).unwrap_err(), ProcessError::Tiny);
    }
}
