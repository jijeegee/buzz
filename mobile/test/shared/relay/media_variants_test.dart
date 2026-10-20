import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/media_variants.dart';

const _hash =
    '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
const _png = 'https://relay.example/media/$_hash.png';
const _small = 'https://relay.example/media/$_hash.small.jpg';
const _thumb = 'https://relay.example/media/$_hash.thumb.jpg';

void main() {
  group('relayMediaVariantUrl', () {
    test('maps JPEG and PNG uploads to their variants on the same host', () {
      expect(relayMediaVariantUrl(_png, RelayMediaVariant.thumb), _thumb);
      expect(relayMediaVariantUrl(_png, RelayMediaVariant.small), _small);
      expect(
        relayMediaVariantUrl(
          ' http://127.0.0.1:3000/media/$_hash.JPEG ',
          RelayMediaVariant.thumb,
        ),
        'http://127.0.0.1:3000/media/$_hash.thumb.jpg',
      );
    });

    test('keeps the original for anything without a still variant', () {
      for (final url in [
        null,
        '',
        'https://relay.example/media/$_hash.gif',
        'https://relay.example/media/$_hash.webp',
        'https://relay.example/media/$_hash',
        'https://relay.example/media/$_hash.thumb.jpg',
        'https://relay.example/media/$_hash.png?x=1',
        'https://relay.example/media/$_hash.png#kuzz-anim=x',
        'https://relay.example/files/$_hash.png',
        'https://relay.example/media/abc123.png',
        'data:image/png;base64,AA==',
        'ftp://relay.example/media/$_hash.png',
      ]) {
        expect(
          relayMediaVariantUrl(url, RelayMediaVariant.thumb),
          isNull,
          reason: '$url',
        );
      }
    });
  });

  group('relayAvatarImageUrls', () {
    List<String> urls(String url, double size, {double ratio = 3}) =>
        relayAvatarImageUrls(url, logicalSize: size, devicePixelRatio: ratio);

    test('row-size avatars try small, then thumb, then the original', () {
      expect(urls(_png, 42), [_small, _thumb, _png]);
      expect(urls(_png, 56), [_small, _thumb, _png]);
    });

    test('larger avatars use the thumb while it stays sharp', () {
      expect(urls(_png, 57), [_thumb, _png]);
      expect(urls(_png, 88), [_thumb, _png]);
      expect(urls(_png, 106, ratio: 3), [_thumb, _png]);
    });

    test('avatars past the thumb resolution load the original', () {
      expect(urls(_png, 120, ratio: 3), [_png]);
      expect(urls(_png, double.infinity), [_png]);
    });

    test('URLs without variants load only the original', () {
      const gif = 'https://relay.example/media/$_hash.gif';
      expect(urls(gif, 42), [gif]);
    });
  });
}
