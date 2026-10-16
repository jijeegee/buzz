import 'dart:convert';

import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

class _FixedRelayConfig extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: 'https://relay.example');
}

void main() {
  test('fetchRelayInfo reads supported_extensions from NIP-11', () async {
    late http.Request seen;
    final client = MockClient((request) async {
      seen = request;
      return http.Response(
        jsonEncode({
          'name': 'relay',
          'supported_extensions': ['nip-cw-newer', 7, 'nip-pl'],
        }),
        200,
      );
    });
    final info = await fetchRelayInfo(
      'https://relay.example/x',
      client: client,
    );
    expect(seen.url.toString(), 'https://relay.example/');
    expect(seen.headers['Accept'], 'application/nostr+json');
    expect(info.supports('nip-cw-newer'), isTrue);
    expect(info.supportedExtensions, {'nip-cw-newer', 'nip-pl'});

    final bare = await fetchRelayInfo(
      'https://relay.example',
      client: MockClient((_) async => http.Response('{}', 200)),
    );
    expect(bare.supports('nip-cw-newer'), isFalse);
    await expectLater(
      fetchRelayInfo(
        'https://relay.example',
        client: MockClient((_) async => http.Response('nope', 503)),
      ),
      throwsStateError,
    );
  });

  test('relayInfoProvider is null when the document is unreadable', () async {
    var fail = true;
    final container = ProviderContainer(
      overrides: [
        relayConfigProvider.overrideWith(_FixedRelayConfig.new),
        relayInfoFetcherProvider.overrideWithValue((baseUrl) async {
          expect(baseUrl, 'https://relay.example');
          if (fail) throw StateError('offline');
          return const RelayInfo(supportedExtensions: {'nip-cw-newer'});
        }),
      ],
    );
    addTearDown(container.dispose);

    expect(await container.read(relayInfoProvider.future), isNull);
    fail = false;
    container.invalidate(relayInfoProvider);
    final info = await container.read(relayInfoProvider.future);
    expect(info?.supports('nip-cw-newer'), isTrue);
  });
}
