import 'dart:typed_data';

import 'package:image/image.dart' as image;

/// Largest side, in pixels, of an uploaded group or profile photo.
const avatarUploadMaxPixels = 640;

const _avatarJpegQuality = 85;

/// Encodes a cropped group or profile photo for upload.
///
/// Shrinks it to at most [avatarUploadMaxPixels] on its longer side and never
/// enlarges it. The result is a JPEG, unless the image has transparent
/// pixels: then it stays a PNG so the transparency survives.
Uint8List encodeAvatarForUpload(image.Image source) {
  final longest = source.width >= source.height ? source.width : source.height;
  final resized = longest <= avatarUploadMaxPixels
      ? source
      : source.width >= source.height
      ? image.copyResize(
          source,
          width: avatarUploadMaxPixels,
          interpolation: image.Interpolation.cubic,
        )
      : image.copyResize(
          source,
          height: avatarUploadMaxPixels,
          interpolation: image.Interpolation.cubic,
        );
  if (_hasTransparentPixel(resized)) return image.encodePng(resized);
  return image.encodeJpg(resized, quality: _avatarJpegQuality);
}

bool _hasTransparentPixel(image.Image source) {
  if (!source.hasAlpha) return false;
  for (final pixel in source) {
    if (pixel.a < pixel.maxChannelValue) return true;
  }
  return false;
}

/// Whether [bytes] are a GIF. GIF photos skip cropping and resizing and are
/// uploaded as they are, so their animation is kept.
bool isGifImage(Uint8List bytes) =>
    bytes.length >= 4 &&
    bytes[0] == 0x47 && // G
    bytes[1] == 0x49 && // I
    bytes[2] == 0x46 && // F
    bytes[3] == 0x38; // 8

/// The upload MIME type of photo [bytes] prepared for a group or profile:
/// GIF and PNG by signature, otherwise JPEG.
String avatarUploadMimeType(Uint8List bytes) {
  if (isGifImage(bytes)) return 'image/gif';
  const png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
  if (bytes.length >= png.length) {
    var isPng = true;
    for (var i = 0; i < png.length; i++) {
      if (bytes[i] != png[i]) {
        isPng = false;
        break;
      }
    }
    if (isPng) return 'image/png';
  }
  return 'image/jpeg';
}
