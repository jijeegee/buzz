import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:image/image.dart' as image;
import 'package:kuzz/features/profile/avatar_upload_image.dart';

image.Image _square(int size, {bool transparent = false}) {
  final source = image.Image(
    width: size,
    height: size,
    numChannels: transparent ? 4 : 3,
  );
  image.fill(
    source,
    color: transparent
        ? image.ColorRgba8(200, 40, 40, 0)
        : image.ColorRgb8(200, 40, 40),
  );
  return source;
}

void main() {
  test('shrinks a large photo to 640 pixels as a JPEG', () {
    final bytes = encodeAvatarForUpload(_square(2048));
    final decoded = image.decodeImage(bytes)!;

    expect(avatarUploadMimeType(bytes), 'image/jpeg');
    expect(decoded.width, avatarUploadMaxPixels);
    expect(decoded.height, avatarUploadMaxPixels);
    expect(avatarUploadMaxPixels, 640);
  });

  test('never enlarges a small photo', () {
    final bytes = encodeAvatarForUpload(_square(200));
    final decoded = image.decodeImage(bytes)!;

    expect(decoded.width, 200);
    expect(decoded.height, 200);
  });

  test('keeps a photo with transparency as a PNG', () {
    final bytes = encodeAvatarForUpload(_square(1000, transparent: true));
    final decoded = image.decodeImage(bytes)!;

    expect(avatarUploadMimeType(bytes), 'image/png');
    expect(decoded.width, avatarUploadMaxPixels);
    expect(decoded.getPixel(0, 0).a, 0);
  });

  test('an opaque alpha channel still encodes as a JPEG', () {
    final source = image.Image(width: 10, height: 10, numChannels: 4);
    image.fill(source, color: image.ColorRgba8(1, 2, 3, 255));

    expect(avatarUploadMimeType(encodeAvatarForUpload(source)), 'image/jpeg');
  });

  test('recognizes GIFs by signature so they skip cropping', () {
    final gif = image.encodeGif(_square(4));

    expect(isGifImage(gif), isTrue);
    expect(avatarUploadMimeType(gif), 'image/gif');
    expect(isGifImage(Uint8List.fromList(image.encodePng(_square(4)))), false);
  });
}
