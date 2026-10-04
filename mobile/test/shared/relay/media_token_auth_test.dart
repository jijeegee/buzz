import 'dart:convert';
import 'dart:typed_data';

import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;

import 'fake_relay_access_tokens.dart';

final _pngBytes = Uint8List.fromList([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, //
  0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
]);

http.Response _descriptor() => http.Response(
  jsonEncode({
    'url': 'https://relay.example/media/a.png',
    'sha256':
        '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
    'size': 16,
    'type': 'image/png',
    'uploaded': 1,
  }),
  200,
);

http.Response _expired() => http.Response(
  jsonEncode({'error': 'authentication failed', 'code': 'token_expired'}),
  401,
);

class _TokenConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(
    baseUrl: 'https://relay.example',
    tokenAuth: true,
    principalId:
        'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22',
  );
}

void main() {
  group('Blossom upload on a token community', () {
    test('PUTs with Bearer and retries one token_expired', () async {
      final tokens = FakeRelayAccessTokens(
        token: 'bzs_1',
        rotations: ['bzs_2'],
      );
      final requests = <http.Request>[];
      final responses = [_expired(), _descriptor()];
      final service = MediaUploadService(
        baseUrl: 'https://relay.example',
        nsec: null,
        accessTokens: tokens,
        httpClient: http_testing.MockClient((request) async {
          requests.add(request);
          return responses.removeAt(0);
        }),
        pickGalleryVideo: () async => null,
        pickGalleryImage: () async => null,
      );

      final blob = await service.uploadBytes(_pngBytes, mimeType: 'image/png');

      expect(blob.url, 'https://relay.example/media/a.png');
      expect(requests.map((r) => r.headers['Authorization']), [
        'Bearer bzs_1',
        'Bearer bzs_2',
      ]);
      expect(requests.last.method, 'PUT');
      expect(requests.last.headers['X-SHA-256'], hasLength(64));
      expect(requests.last.bodyBytes, _pngBytes);
    });

    test('no token fails before any request', () async {
      var calls = 0;
      final service = MediaUploadService(
        baseUrl: 'https://relay.example',
        nsec: null,
        accessTokens: FakeRelayAccessTokens(token: null),
        httpClient: http_testing.MockClient((request) async {
          calls++;
          return _descriptor();
        }),
        pickGalleryVideo: () async => null,
        pickGalleryImage: () async => null,
      );
      await expectLater(
        service.uploadBytes(_pngBytes, mimeType: 'image/png'),
        throwsA(isA<RelayTokenUnavailableException>()),
      );
      expect(calls, 0);
    });
  });

  group('media GET headers on a token community', () {
    test('relay media URLs get the current bearer, read per request', () {
      final tokens = FakeRelayAccessTokens(token: 'bzs_1');
      final container = ProviderContainer(
        overrides: [
          relayConfigProvider.overrideWith(_TokenConfigNotifier.new),
          relayAccessTokensProvider.overrideWithValue(tokens),
        ],
      );
      addTearDown(container.dispose);
      final auth = container.read(mediaGetAuthServiceProvider);

      expect(auth.headersFor('https://relay.example/media/a.png'), {
        'Authorization': 'Bearer bzs_1',
      });
      tokens.token = 'bzs_2';
      expect(auth.headersFor('https://relay.example/media/a.png'), {
        'Authorization': 'Bearer bzs_2',
      });
      expect(
        auth.headersFor('https://cdn.example/media/a.png'),
        isEmpty,
        reason: 'never leak the bearer to another host',
      );
      expect(auth.headersFor('https://relay.example/other'), isEmpty);
      tokens.token = null;
      expect(auth.headersFor('https://relay.example/media/a.png'), isEmpty);
    });
  });
}
