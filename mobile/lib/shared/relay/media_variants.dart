/// Largest logical size, in points, that draws the relay's small variant.
const relaySmallVariantMaxLogical = 56.0;

/// The largest physical size, in pixels, the relay's thumbnail serves sharply.
///
/// The relay stores one JPEG thumbnail per uploaded still image, at most 320
/// pixels on its longer side (`kuzz-media` `thumbnail.rs`).
const relayThumbnailMaxPixels = 320.0;

/// A resized copy the relay serves next to an uploaded still image.
enum RelayMediaVariant {
  /// `/media/<sha256>.small.jpg`, 160 pixels, made on first request.
  small('small'),

  /// `/media/<sha256>.thumb.jpg`, 320 pixels, made at upload.
  thumb('thumb');

  const RelayMediaVariant(this.suffix);

  /// The path segment between the hash and `.jpg`.
  final String suffix;
}

final _mediaBlobPath = RegExp(r'^/media/([0-9a-f]{64})\.([A-Za-z0-9]{1,8})$');

/// Extensions whose relay variants are faithful still previews. GIF and WebP
/// may be animated, and a JPEG variant would freeze the first frame.
const _stillImageExtensions = {'jpg', 'jpeg', 'png'};

/// The relay's [variant] of the still image at [url], or null when it has
/// none worth using.
///
/// A relay upload lives at `/media/<sha256>.<ext>` and its variants at
/// `/media/<sha256>.<variant>.jpg` on the same host. Only JPEG and PNG
/// uploads map to a variant; anything else (GIF, WebP, URLs with a query or
/// fragment, non-media paths) returns null so callers keep the original.
String? relayMediaVariantUrl(String? url, RelayMediaVariant variant) {
  final trimmed = url?.trim();
  if (trimmed == null || trimmed.isEmpty) return null;
  final uri = Uri.tryParse(trimmed);
  if (uri == null ||
      (uri.scheme != 'https' && uri.scheme != 'http') ||
      uri.host.isEmpty ||
      uri.hasQuery ||
      uri.hasFragment) {
    return null;
  }
  final match = _mediaBlobPath.firstMatch(uri.path);
  if (match == null) return null;
  if (!_stillImageExtensions.contains(match.group(2)!.toLowerCase())) {
    return null;
  }
  return uri
      .replace(path: '/media/${match.group(1)}.${variant.suffix}.jpg')
      .toString();
}

/// The URLs to try, in order, for an avatar of [url] drawn [logicalSize]
/// points across at [devicePixelRatio]. The original [url] is always last.
///
/// - Up to [relaySmallVariantMaxLogical] points: small, then thumb.
/// - Up to [relayThumbnailMaxPixels] physical pixels: thumb.
/// - Larger, or a size that is not finite: the original only.
///
/// Each step is the fallback for the one before when it fails to load, as a
/// variant does for uploads older than it.
List<String> relayAvatarImageUrls(
  String url, {
  required double logicalSize,
  required double devicePixelRatio,
}) {
  final thumb = relayMediaVariantUrl(url, RelayMediaVariant.thumb);
  if (thumb == null || !logicalSize.isFinite || logicalSize <= 0) {
    return [url];
  }
  if (logicalSize <= relaySmallVariantMaxLogical) {
    return [relayMediaVariantUrl(url, RelayMediaVariant.small)!, thumb, url];
  }
  if (logicalSize * devicePixelRatio <= relayThumbnailMaxPixels) {
    return [thumb, url];
  }
  return [url];
}
