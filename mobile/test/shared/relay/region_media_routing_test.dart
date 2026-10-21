import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/features/groups/groups_api.dart';
import 'package:kuzz/shared/auth/default_community.dart'
    show rememberConnectUrl;
import 'package:kuzz/shared/huddle/huddle.dart';
import 'package:kuzz/shared/relay/connect_url_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

import 'fake_relay_access_tokens.dart';

/// Media, group requests and huddle audio go to the account's home region
/// once the service has given one; auth stays made for the service address.
void main() {
  const service = 'https://relay.example';
  const principal =
      'cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33';
  const config = RelayConfig(
    baseUrl: service,
    tokenAuth: true,
    principalId: principal,
  );
  const media = '$service/media/a.png';

  setUp(() => rememberConnectUrl(principal, 'wss://jp.relay.example'));
  tearDown(() => rememberConnectUrl(principal, null));

  test('service addresses map to the region, others stay', () {
    expect(regionUrlFor(config, media), 'https://jp.relay.example/media/a.png');
    expect(regionSocketUrlFor(config, config.wsUrl), 'wss://jp.relay.example');
    expect(
      regionUrlFor(config, 'https://cdn.example/media/a.png'),
      'https://cdn.example/media/a.png',
    );
    const keyAccount = RelayConfig(baseUrl: service, nsec: null);
    expect(regionUrlFor(keyAccount, media), media);
    rememberConnectUrl(principal, null);
    expect(regionUrlFor(config, media), media);
    expect(regionSocketUrlFor(config, config.wsUrl), config.wsUrl);
  });

  test('media is fetched from the region with the service bearer', () async {
    final auth = MediaGetAuthService(
      baseUrl: service,
      nsec: null,
      accessTokens: FakeRelayAccessTokens(token: 'bzs_1'),
      route: (url) => regionUrlFor(config, url),
    );
    expect(auth.resolve(media), 'https://jp.relay.example/media/a.png');
    expect(
      auth.resolve('https://cdn.example/a.png'),
      'https://cdn.example/a.png',
    );
    // Either form gets the bearer (a native view may hand back the routed
    // one); another host never does.
    expect(auth.headersFor(media), {'Authorization': 'Bearer bzs_1'});
    expect(auth.headersFor(auth.resolve(media)), {
      'Authorization': 'Bearer bzs_1',
    });
    expect(auth.headersFor('https://cdn.example/media/a.png'), isEmpty);

    final requests = <http.Request>[];
    await auth.get(
      http_testing.MockClient((request) async {
        requests.add(request);
        return http.Response('', 200);
      }),
      media,
    );
    expect(requests.single.url.toString(), auth.resolve(media));
    expect(requests.single.headers['Authorization'], 'Bearer bzs_1');
  });

  test('uploads go to the region; the descriptor stays as served', () async {
    final requests = <http.Request>[];
    final upload = MediaUploadService(
      baseUrl: service,
      nsec: null,
      accessTokens: FakeRelayAccessTokens(token: 'bzs_1'),
      route: (url) => regionUrlFor(config, url),
      httpClient: http_testing.MockClient((request) async {
        requests.add(request);
        return http.Response(
          jsonEncode({
            'url': media,
            'sha256':
                '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
            'size': 16,
            'type': 'image/png',
            'uploaded': 1,
          }),
          200,
        );
      }),
      pickGalleryVideo: () async => null,
      pickGalleryImage: () async => null,
    );
    final blob = await upload.uploadBytes(
      Uint8List.fromList([
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, //
        0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
      ]),
      mimeType: 'image/png',
    );
    expect(requests.single.url.host, 'jp.relay.example');
    expect(requests.single.headers['Authorization'], 'Bearer bzs_1');
    expect(blob.url, media);
  });

  test('group requests go to the region', () async {
    final requests = <http.Request>[];
    final api = GroupsApi(
      client: http_testing.MockClient((request) async {
        requests.add(request);
        return http.Response(jsonEncode({'groups': []}), 200);
      }),
      config: config,
      accessTokens: FakeRelayAccessTokens(token: 'bzs_1'),
    );
    await api.search('design');
    expect(
      requests.single.url.toString(),
      'https://jp.relay.example/api/groups/search?q=design&limit=20',
    );
    expect(requests.single.headers['Authorization'], 'Bearer bzs_1');
  });

  test('huddle audio connects to the region, AUTH names the service', () {
    final parameters = HuddleConnectionParameters(
      relayWebSocketUrl: config.wsUrl,
      audioRelayUrl: regionSocketUrlFor(config, config.wsUrl),
      accessTokens: FakeRelayAccessTokens(token: 'bzs_1'),
      parentChannelId: '11111111-1111-4111-8111-111111111111',
      ephemeralChannelId: '22222222-2222-4222-8222-222222222222',
    );
    expect(
      parameters.audioWebSocketUri.toString(),
      'wss://jp.relay.example/huddle/22222222-2222-4222-8222-222222222222/audio',
    );
    expect(parameters.relayWebSocketUrl, 'wss://relay.example');
  });
}
