import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/shared/relay/media_auth.dart';
import 'package:kuzz/shared/relay/media_image.dart';
import 'package:kuzz/shared/widgets/avatar_image.dart';

// Minimal valid 1x1 PNG.
final _pngBytes = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAA'
  'AAYAAjCB0C8AAAAASUVORK5CYII=',
);

const _hash =
    'fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210';
const _original = 'https://relay.example/media/$_hash.png';
const _small = 'https://relay.example/media/$_hash.small.jpg';
const _thumb = 'https://relay.example/media/$_hash.thumb.jpg';

void main() {
  setUp(() {
    MediaImageProvider.debugResetCooldowns();
    PaintingBinding.instance.imageCache.clear();
    PaintingBinding.instance.imageCache.clearLiveImages();
  });

  Future<List<String>> pumpAvatar(
    WidgetTester tester, {
    required double radius,
    required Set<String> available,
  }) async {
    final requested = <String>[];
    final client = http_testing.MockClient((request) async {
      final url = request.url.toString();
      requested.add(url);
      return available.contains(url)
          ? http.Response.bytes(_pngBytes, 200)
          : http.Response('', 404);
    });
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          mediaGetAuthServiceProvider.overrideWithValue(
            MediaGetAuthService(baseUrl: 'https://relay.example', nsec: null),
          ),
          mediaHttpClientProvider.overrideWithValue(client),
        ],
        child: MaterialApp(
          home: MediaQuery(
            data: const MediaQueryData(devicePixelRatio: 3),
            child: Center(
              child: AvatarImage(
                imageUrl: _original,
                radius: radius,
                fallback: const Text('R'),
              ),
            ),
          ),
        ),
      ),
    );
    for (var i = 0; i < 4; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 20)),
      );
      await tester.pump();
    }
    return requested;
  }

  testWidgets('a row-size avatar loads only the small copy', (tester) async {
    final requested = await pumpAvatar(
      tester,
      radius: 21,
      available: {_small, _thumb, _original},
    );

    expect(requested, [_small]);
    expect(find.text('R'), findsNothing);
  });

  testWidgets('a missing small copy falls back to the thumb, then original', (
    tester,
  ) async {
    final requested = await pumpAvatar(
      tester,
      radius: 21,
      available: {_original},
    );

    expect(requested, [_small, _thumb, _original]);
    expect(find.text('R'), findsNothing);
  });

  testWidgets('a header-size avatar starts at the thumb', (tester) async {
    final requested = await pumpAvatar(
      tester,
      radius: 40,
      available: {_thumb, _original},
    );

    expect(requested, [_thumb]);
  });

  testWidgets('the letter shows when no copy loads', (tester) async {
    final requested = await pumpAvatar(tester, radius: 21, available: {});

    expect(requested, [_small, _thumb, _original]);
    expect(find.text('R'), findsOneWidget);
  });
}
