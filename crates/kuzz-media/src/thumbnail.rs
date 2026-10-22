//! Synchronous thumbnail generation and blurhash encoding.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat};

use crate::config::MediaConfig;
use crate::error::MediaError;
use crate::storage::BlobMeta;

/// Longest side of the `{sha256}.thumb.jpg` variant, written at upload.
pub const THUMB_MAX_DIM: u32 = 320;

/// Longest side of the `{sha256}.small.jpg` variant (list avatars), rendered
/// on first read.
pub const SMALL_MAX_DIM: u32 = 160;

/// Image MIME types the decoder handles; anything else has no derived variants.
pub fn is_renderable_image(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

/// Composite an image with transparency onto white. JPEG has no alpha, and
/// dropping the channel would leave transparent areas black.
fn flatten_onto_white(img: &DynamicImage) -> DynamicImage {
    if !img.color().has_alpha() {
        return img.clone();
    }
    let rgba = img.to_rgba8();
    let rgb = image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let a = u16::from(a);
        let over_white = |c: u8| ((u16::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
        image::Rgb([over_white(r), over_white(g), over_white(b)])
    });
    DynamicImage::ImageRgb8(rgb)
}

/// Encode `img` as JPEG, transparency flattened onto white.
fn encode_jpeg(img: &DynamicImage) -> Result<Vec<u8>, MediaError> {
    let mut bytes = Vec::new();
    flatten_onto_white(img).write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)?;
    Ok(bytes)
}

/// Render the `{sha256}.small.jpg` variant from the original image bytes
/// (CPU-bound, sync): the upload thumbnail pipeline at [`SMALL_MAX_DIM`],
/// aspect kept and never upscaled. Animated GIFs use their first frame.
pub fn render_small_jpeg_sync(bytes: &[u8]) -> Result<Vec<u8>, MediaError> {
    let img = image::load_from_memory(bytes)?;
    if img.width() <= SMALL_MAX_DIM && img.height() <= SMALL_MAX_DIM {
        return encode_jpeg(&img);
    }
    encode_jpeg(&img.thumbnail(SMALL_MAX_DIM, SMALL_MAX_DIM))
}

/// Generate thumbnail and blurhash from image bytes (CPU-bound, sync).
///
/// Returns `(metadata, optional thumbnail JPEG bytes)`.
/// Caller handles S3 writes after `spawn_blocking` returns.
pub fn generate_image_metadata_sync(
    config: &MediaConfig,
    sha256: &str,
    bytes: &[u8],
    mime: &str,
    ext: &str,
) -> Result<(BlobMeta, Option<Vec<u8>>), MediaError> {
    if !mime.starts_with("image/") {
        return Ok((BlobMeta::default(), None));
    }

    let img = image::load_from_memory(bytes)?;
    let (w, h) = (img.width(), img.height());

    // Thumbnail: 320px max dimension, preserve aspect ratio
    let thumb = img.thumbnail(THUMB_MAX_DIM, THUMB_MAX_DIM);
    let thumb_bytes = encode_jpeg(&thumb)?;

    // Blurhash from thumbnail (faster than full image)
    let rgba = thumb.to_rgba8();
    let bh =
        blurhash::encode(4, 3, thumb.width(), thumb.height(), rgba.as_raw()).unwrap_or_default();

    Ok((
        BlobMeta {
            dim: format!("{w}x{h}"),
            blurhash: bh,
            thumb_url: format!("{}/{sha256}.thumb.jpg", config.public_base_url),
            ext: ext.to_string(),
            mime_type: mime.to_string(),
            size: bytes.len() as u64,
            ..BlobMeta::default() // uploaded_at set by caller (process_upload)
        },
        Some(thumb_bytes),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(img: DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), format)
            .expect("encode fixture");
        bytes
    }

    fn rendered_dims(bytes: &[u8]) -> (u32, u32) {
        let small = render_small_jpeg_sync(bytes).expect("render small");
        assert_eq!(
            image::guess_format(&small).expect("format"),
            ImageFormat::Jpeg
        );
        let img = image::load_from_memory(&small).expect("decode small");
        (img.width(), img.height())
    }

    #[test]
    fn small_variant_fits_160_and_keeps_aspect() {
        let wide = encode(DynamicImage::new_rgb8(640, 320), ImageFormat::Png);
        assert_eq!(rendered_dims(&wide), (160, 80));
        let tall = encode(DynamicImage::new_rgb8(300, 1200), ImageFormat::Jpeg);
        assert_eq!(rendered_dims(&tall), (40, 160));
        let square = encode(DynamicImage::new_rgba8(2000, 2000), ImageFormat::Png);
        assert_eq!(rendered_dims(&square), (160, 160));
    }

    #[test]
    fn small_variant_never_upscales() {
        let tiny = encode(DynamicImage::new_rgb8(64, 48), ImageFormat::Png);
        assert_eq!(rendered_dims(&tiny), (64, 48));
    }

    #[test]
    fn small_variant_renders_gif_and_webp() {
        let gif = encode(DynamicImage::new_rgba8(400, 200), ImageFormat::Gif);
        assert_eq!(rendered_dims(&gif), (160, 80));
        let webp = encode(DynamicImage::new_rgba8(200, 400), ImageFormat::WebP);
        assert_eq!(rendered_dims(&webp), (80, 160));
    }

    /// Transparent pixels become white, not black, in both JPEG variants;
    /// opaque pixels keep their colour.
    #[test]
    fn transparency_is_flattened_onto_white() {
        let mut img = image::RgbaImage::from_pixel(400, 400, image::Rgba([0, 0, 0, 0]));
        for x in 0..200 {
            for y in 0..400 {
                img.put_pixel(x, y, image::Rgba([200, 30, 30, 255]));
            }
        }
        let png = encode(DynamicImage::ImageRgba8(img), ImageFormat::Png);
        let check = |jpeg: &[u8], w: u32| {
            let out = image::load_from_memory(jpeg).expect("decode").to_rgb8();
            let clear = out.get_pixel(w * 7 / 8, w / 2).0;
            assert!(
                clear.iter().all(|&c| c >= 245),
                "transparent -> white: {clear:?}"
            );
            let opaque = out.get_pixel(w / 8, w / 2).0;
            assert!(
                opaque[0] > 170 && opaque[1] < 70 && opaque[2] < 70,
                "opaque kept: {opaque:?}"
            );
        };
        check(&render_small_jpeg_sync(&png).expect("small"), SMALL_MAX_DIM);
        let config = MediaConfig {
            s3_endpoint: String::new(),
            s3_access_key: String::new(),
            s3_secret_key: String::new(),
            s3_bucket: String::new(),
            s3_region: "us-east-1".to_string(),
            s3_addressing_style: crate::config::S3AddressingStyle::Path,
            max_image_bytes: 1 << 20,
            max_gif_bytes: 1 << 20,
            max_video_bytes: 1 << 20,
            max_file_bytes: 1 << 20,
            public_base_url: String::new(),
            upload_records_enabled: false,
            upload_ip_header: None,
            upload_port_header: None,
        };
        let (_, thumb) =
            generate_image_metadata_sync(&config, "x", &png, "image/png", "png").expect("thumb");
        check(&thumb.expect("thumb bytes"), THUMB_MAX_DIM);
    }

    #[test]
    fn small_variant_rejects_non_images() {
        assert!(render_small_jpeg_sync(b"%PDF-1.7 not an image").is_err());
        assert!(is_renderable_image("image/png"));
        assert!(is_renderable_image("image/gif"));
        assert!(!is_renderable_image("image/svg+xml"));
        assert!(!is_renderable_image("video/mp4"));
    }
}
