import 'dart:convert';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:kuzz/shared/push/android_push_enrollment.dart';
import 'package:kuzz/shared/push/push_transcript.dart';

import '../community/community_storage_test.dart' show FakeSecureStorage;

const _gateway = 'https://push.example';
const _relay = 'wss://relay.example';
final _relayKeyA = 'a' * 64;
final _relayKeyB = 'b' * 64;
const _now = 1800000000;

/// Stage B A4.1: the Android enrollment against a scripted gateway.
void main() {
  late _Gateway gateway;
  late _Keys keys;
  late AndroidPushEnrollmentStore store;
  late int ids;

  AndroidPushEnrollment enrollment() => AndroidPushEnrollment(
    gatewayUrl: _gateway,
    client: gateway.client,
    store: store,
    keys: keys,
    now: () => DateTime.fromMillisecondsSinceEpoch(_now * 1000),
    randomHex: (bytes) => (ids++).toRadixString(16).padLeft(bytes * 2, '0'),
  );

  setUp(() {
    gateway = _Gateway();
    keys = _Keys();
    store = AndroidPushEnrollmentStore(FakeSecureStorage());
    ids = 1;
  });

  test('a fresh token enrolls, attests and delegates', () async {
    final grant = await enrollment().enroll('tok-1', _relay);

    expect(gateway.routes, [
      'NIP-11',
      'v1/installations/challenges',
      'v1/installations',
      'v1/installations/challenges',
      'v1/delegations',
    ]);
    final install = gateway.bodies['v1/installations']!.single;
    final endpoint = utf8
        .encode('tok-1')
        .map((b) => b.toRadixString(16).padLeft(2, '0'))
        .join();
    expect(install['app_profile'], 'kuzz-android');
    expect(install['endpoint'], endpoint);
    expect(install['attestation'], keys.chain);
    expect(
      install['key_id'],
      base64.encode(sha256.convert(keys.publicKey).bytes),
    );
    expect(install['expires_at'], _now + 2592000);
    // The key's challenge: the enrollment transcript with an empty key_id.
    final expectedChallenge = sha256.convert(
      KuzzPushTranscript.enroll(
        gatewayOrigin: _gateway,
        challengeId: install['challenge_id'] as String,
        challenge: install['challenge'] as String,
        keyId: '',
        appProfile: 'kuzz-android',
        endpoint: endpoint,
        endpointEpoch: 1,
        expiresAt: _now + 2592000,
      ),
    );
    expect(keys.challenges.single, expectedChallenge.bytes);

    final delegate = gateway.bodies['v1/delegations']!.single;
    expect(delegate['relay_pubkey'], _relayKeyA);
    expect(delegate['generation'], 1);
    expect(delegate['installation_handle'], gateway.handle);
    expect(
      base64.decode(delegate['assertion'] as String),
      keys.signatureOf(
        KuzzPushTranscript.delegate(
          gatewayOrigin: _gateway,
          challengeId: delegate['challenge_id'] as String,
          challenge: delegate['challenge'] as String,
          installationHandle: gateway.handle,
          endpointEpoch: 1,
          generation: 1,
          relayPubkey: _relayKeyA,
          notBefore: _now,
          expiresAt: _now + 2592000,
        ),
      ),
    );
    expect(grant.endpointGrant, 'grant-1');
    expect(grant.endpointHash, sha256.convert(utf8.encode('tok-1')).toString());
    expect(grant.toGrantMap()['appProfile'], 'kuzz-android');
    expect(grant.relayOrigin, _relay);
    expect((await store.load()).pending, isEmpty);
  });

  test('the same token reuses the stored grant', () async {
    final first = await enrollment().enroll('tok-1', _relay);
    gateway.routes.clear();
    final second = await enrollment().enroll('tok-1', _relay);
    expect(gateway.routes, ['NIP-11']);
    expect(second.endpointGrant, first.endpointGrant);
    expect(second.installationId, first.installationId);
  });

  test('a refreshed token enrolls afresh (S9)', () async {
    await enrollment().enroll('tok-1', _relay);
    gateway.routes.clear();
    final grant = await enrollment().enroll('tok-2', _relay);
    expect(gateway.routes, contains('v1/installations'));
    expect(keys.challenges, hasLength(2));
    expect(grant.endpointHash, sha256.convert(utf8.encode('tok-2')).toString());
  });

  test('a rotated relay key delegates without enrolling again', () async {
    await enrollment().enroll('tok-1', _relay);
    gateway
      ..routes.clear()
      ..relayKey = _relayKeyB;
    final grant = await enrollment().enroll('tok-1', _relay);
    expect(gateway.routes, [
      'NIP-11',
      'v1/installations/challenges',
      'v1/delegations',
    ]);
    expect(grant.relayPubkey, _relayKeyB);
    expect(grant.generation, 1);
    expect(keys.challenges, hasLength(1));
  });

  test('a failed delegation resumes at a higher generation', () async {
    gateway.fail['v1/delegations'] = 500;
    await expectLater(
      enrollment().enroll('tok-1', _relay),
      throwsA(isA<GatewayStatusException>()),
    );
    expect((await store.load()).pending, hasLength(1));
    gateway
      ..fail.clear()
      ..routes.clear();
    final grant = await enrollment().enroll('tok-1', _relay);
    expect(gateway.routes, isNot(contains('v1/installations')));
    expect(grant.generation, 2);
    expect(keys.challenges, hasLength(1));
  });

  test('a 404 replay drops the journal and enrolls again', () async {
    gateway.fail['v1/installations'] = 500;
    await expectLater(
      enrollment().enroll('tok-1', _relay),
      throwsA(isA<GatewayStatusException>()),
    );
    final journaled = (await store.load()).pending.values.single as Map;
    gateway.fail['v1/installations'] = 404;
    gateway.failOnce = true;
    final grant = await enrollment().enroll('tok-1', _relay);
    final installs = gateway.bodies['v1/installations']!;
    // The exact journaled request was replayed first.
    expect(installs[1]['challenge_id'], journaled['challengeId']);
    expect(installs[2]['challenge_id'], isNot(journaled['challengeId']));
    expect(keys.challenges, hasLength(2));
    expect(grant.generation, 1);
  });

  test('other failures keep the journal', () async {
    gateway.fail['v1/installations'] = 500;
    for (var i = 0; i < 2; i++) {
      await expectLater(
        enrollment().enroll('tok-1', _relay),
        throwsA(isA<GatewayStatusException>()),
      );
    }
    expect((await store.load()).pending, hasLength(1));
    expect(keys.challenges, hasLength(1));
  });

  test('NIP-11 must name exactly one current push key', () async {
    gateway.extraCurrentKey = true;
    await expectLater(
      enrollment().enroll('tok-1', _relay),
      throwsFormatException,
    );
    expect(gateway.routes, ['NIP-11']);
  });

  test('tokens over 512 bytes are refused before any call (N11)', () async {
    expect(fcmEndpoint('x' * 512), isNotNull);
    expect(fcmEndpoint('x' * 513), isNull);
    expect(fcmEndpoint('has space'), isNull);
    expect(fcmEndpoint(''), isNull);
    await expectLater(
      enrollment().enroll('x' * 513, _relay),
      throwsFormatException,
    );
    expect(gateway.routes, isEmpty);
  });
}

class _Keys implements AndroidEnrollmentKeys {
  final challenges = <List<int>>[];
  final chain = base64.encode(utf8.encode('chain'));
  final publicKey = utf8.encode('spki');

  List<int> signatureOf(List<int> data) => sha256.convert(data).bytes;

  @override
  Future<({String chain, List<int> publicKey})> create(
    String alias,
    List<int> challenge,
  ) async {
    expect(alias, startsWith('kuzz.push.enroll.'));
    challenges.add(challenge);
    return (chain: chain, publicKey: publicKey);
  }

  @override
  Future<List<int>> sign(String alias, List<int> data) async =>
      signatureOf(data);
}

class _Gateway {
  final routes = <String>[];
  final bodies = <String, List<Map<String, dynamic>>>{};
  final fail = <String, int>{};
  bool failOnce = false;
  String relayKey = 'a' * 64;
  bool extraCurrentKey = false;
  final handle = '22222222-2222-4222-8222-222222222222';
  var _challenges = 0;
  var _grants = 0;

  late final http.Client client = MockClient((request) async {
    if (request.url.toString() == 'https://relay.example/') {
      routes.add('NIP-11');
      return http.Response(
        jsonEncode({
          'self': 'c' * 64,
          'push': {
            'keys': [
              {'pubkey': relayKey, 'current': true},
              {'pubkey': 'd' * 64, 'current': extraCurrentKey},
            ],
          },
        }),
        200,
      );
    }
    final route = request.url.path.substring(1);
    routes.add(route);
    final body = jsonDecode(request.body) as Map<String, dynamic>;
    bodies.putIfAbsent(route, () => []).add(body);
    final status = fail[route];
    if (status != null) {
      if (failOnce) {
        fail.remove(route);
        failOnce = false;
      }
      return http.Response('{}', status);
    }
    switch (route) {
      case 'v1/installations/challenges':
        _challenges++;
        final bytes = List.filled(32, _challenges);
        return http.Response(
          jsonEncode({
            'challenge_id':
                '11111111-1111-4111-8111-${_challenges.toString().padLeft(12, '0')}',
            'challenge': base64Url.encode(bytes).replaceAll('=', ''),
            'expires_at': _now + 300,
          }),
          200,
        );
      case 'v1/installations':
        return http.Response(
          jsonEncode({
            'installation_handle': handle,
            'endpoint_epoch': body['endpoint_epoch'],
            'expires_at': body['expires_at'],
          }),
          201,
        );
      case 'v1/delegations':
        _grants++;
        return http.Response(
          jsonEncode({'endpoint_grant': 'grant-$_grants'}),
          201,
        );
    }
    return http.Response('{}', 404);
  });
}
