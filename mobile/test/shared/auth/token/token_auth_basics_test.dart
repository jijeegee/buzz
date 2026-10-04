import 'dart:math';

import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import 'token_auth_test_fakes.dart';

void main() {
  group('normalizeRelayOrigin', () {
    test('folds websocket schemes, case, default ports and paths', () {
      expect(
        normalizeRelayOrigin('wss://Relay.Example.com/'),
        'https://relay.example.com',
      );
      expect(
        normalizeRelayOrigin('ws://localhost:3000/x?y#z'),
        'http://localhost:3000',
      );
      expect(
        normalizeRelayOrigin('https://relay.example.com:443'),
        'https://relay.example.com',
      );
      expect(
        normalizeRelayOrigin('http://relay.example.com:80/'),
        'http://relay.example.com',
      );
      expect(
        normalizeRelayOrigin('relay.example.com'),
        'https://relay.example.com',
      );
      expect(normalizeRelayOrigin('http://[::1]:3000'), 'http://[::1]:3000');
    });

    test('keeps distinct origins distinct', () {
      expect(
        normalizeRelayOrigin('https://a.example.com:8443'),
        isNot(normalizeRelayOrigin('https://a.example.com')),
      );
      expect(
        normalizeRelayOrigin('http://a.example.com'),
        isNot(normalizeRelayOrigin('https://a.example.com')),
      );
    });

    test('rejects non-http schemes and missing hosts', () {
      expect(
        () => normalizeRelayOrigin('ftp://x.example'),
        throwsFormatException,
      );
      expect(() => normalizeRelayOrigin('https://'), throwsFormatException);
    });
  });

  group('token auth descriptor', () {
    test('requires bearer exactly true', () {
      expect(parseTokenAuthDescriptor({'name': 'legacy'}), isNull);
      expect(
        parseTokenAuthDescriptor({
          'buzz_token_auth': {'bearer': 'true'},
        }),
        isNull,
      );
      expect(
        parseTokenAuthDescriptor({
          'buzz_token_auth': {'bearer': false},
        }),
        isNull,
      );
      expect(parseTokenAuthDescriptor('not a map'), isNull);
      final descriptor = parseTokenAuthDescriptor({
        'buzz_token_auth': {
          'version': 1,
          'bearer': true,
          'oidc_providers': ['google', 7],
        },
      });
      expect(descriptor, isNotNull);
      expect(descriptor!.oidcProviders, ['google']);
      expect(descriptor.supportsGoogle, isTrue);
    });

    test(
      'fetch asks for NIP-11 and distinguishes legacy from outage',
      () async {
        late http.Request seen;
        final tokenRelay = MockClient((request) async {
          seen = request;
          return jsonResponse({
            'buzz_token_auth': {
              'version': 1,
              'bearer': true,
              'oidc_providers': ['google'],
            },
          });
        });
        final descriptor = await fetchTokenAuthDescriptor(
          'https://r.example',
          tokenRelay,
        );
        expect(descriptor?.supportsGoogle, isTrue);
        expect(seen.url.toString(), 'https://r.example/');
        expect(seen.headers['Accept'], 'application/nostr+json');

        final legacy = MockClient(
          (_) async => jsonResponse({'name': 'legacy'}),
        );
        expect(
          await fetchTokenAuthDescriptor('https://r.example', legacy),
          isNull,
        );

        final down = MockClient((_) async => http.Response('bad gateway', 502));
        await expectLater(
          fetchTokenAuthDescriptor('https://r.example', down),
          throwsA(isA<TokenAuthDetectionException>()),
        );
        final offline = MockClient(
          (_) async => throw http.ClientException('offline'),
        );
        await expectLater(
          fetchTokenAuthDescriptor('https://r.example', offline),
          throwsA(isA<TokenAuthDetectionException>()),
        );
        final garbage = MockClient((_) async => http.Response('<html>', 200));
        await expectLater(
          fetchTokenAuthDescriptor('https://r.example', garbage),
          throwsA(isA<TokenAuthDetectionException>()),
        );
      },
    );
  });

  group('pkce', () {
    test('S256 challenge matches an independent sha256 (openssl)', () {
      // printf '%s' <verifier> | openssl dgst -sha256 -binary | base64url
      expect(
        pkceChallenge('dBjftJeZ4CVP-mJ92IFr1sWHePhszEHN0jeffgcN9Hw'),
        'xUoqklpmeVLTUlDRY8xmYrs8abMQxd70F-Hj0eNzKSM',
      );
    });

    test('generated values fit the relay start-query limits', () {
      final urlSafe = RegExp(r'^[A-Za-z0-9\-._~]+$');
      final pair = PkcePair.generate(Random(1));
      expect(pair.verifier.length, 43);
      expect(pair.challenge.length, 43);
      expect(pair.challenge, pkceChallenge(pair.verifier));
      expect(pair.state.length, inInclusiveRange(16, 256));
      for (final value in [pair.verifier, pair.challenge, pair.state]) {
        expect(urlSafe.hasMatch(value), isTrue, reason: value);
      }
      expect(PkcePair.generate().state, isNot(PkcePair.generate().state));
    });
  });
}
