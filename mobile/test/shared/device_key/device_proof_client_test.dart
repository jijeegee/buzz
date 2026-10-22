import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:kuzz/shared/auth/token/auth_api.dart';
import 'package:kuzz/shared/auth/token/oidc_login.dart';
import 'package:kuzz/shared/device_key/device_proof.dart';
import 'package:kuzz/shared/device_key/device_proof_client.dart';

import '../../helpers/software_device_key.dart';
import '../auth/token/token_auth_test_fakes.dart';

/// Parse and verify a header proof the way the relay does.
bool _proofVerifies(
  SoftwareDeviceKey device,
  String? header, {
  required String purpose,
  required String binding,
  String? token,
  List<int>? body,
}) {
  if (header == null) return false;
  final parts = header.split('.');
  if (parts.length != 3) return false;
  final message = headerProofMessage(
    purpose: purpose,
    binding: binding,
    tokenHash: token == null ? null : sha256Bytes(utf8.encode(token)),
    bodyHash: body == null || body.isEmpty ? null : sha256Bytes(body),
    ts: int.parse(parts[0]),
    nonce: parts[1],
  );
  return verifyDeviceSignature(
    device.publicKey,
    utf8.encode(message),
    hexDecode(parts[2]),
  );
}

void main() {
  late SoftwareDeviceKey device;
  late List<http.Request> sent;

  setUp(() {
    device = SoftwareDeviceKey(List.filled(32, 3));
    sent = [];
    resetServerClockOffset();
  });
  tearDown(resetServerClockOffset);

  DeviceProofClient client(
    http.Response Function(http.Request request) answer,
  ) => DeviceProofClient(
    http_testing.MockClient((request) async {
      sent.add(request);
      return answer(request);
    }),
    deviceKey: device,
  );

  test('a bearer /auth call is proven over URL, token and body', () async {
    final http = client((_) => _ok());
    final body = jsonEncode({'display_name': '한글 😀 a/b'});
    await http.patch(
      Uri.parse('https://relay.example/auth/profile?x=1'),
      headers: {
        'Authorization': 'Bearer bza_token',
        'Content-Type': 'application/json',
      },
      body: body,
    );
    final request = sent.single;
    expect(request.headers['Content-Type'], 'application/json');
    expect(
      _proofVerifies(
        device,
        request.headers[deviceProofHeader],
        purpose: proofPurposeAuth,
        binding: 'PATCH https://relay.example/auth/profile?x=1',
        token: 'bza_token',
        body: utf8.encode(body),
      ),
      isTrue,
    );
  });

  test('a bodyless GET signs "-" for the body', () async {
    final http = client((_) => _ok());
    await http.get(
      Uri.parse('https://relay.example/auth/me'),
      headers: {'Authorization': 'Bearer bza_token'},
    );
    expect(
      _proofVerifies(
        device,
        sent.single.headers[deviceProofHeader],
        purpose: proofPurposeAuth,
        binding: 'GET https://relay.example/auth/me',
        token: 'bza_token',
      ),
      isTrue,
    );
  });

  test('refresh is proven with the refresh token', () async {
    final http = client((_) => _ok());
    final body = jsonEncode({'refresh': 'bzr_refresh'});
    await http.post(
      Uri.parse('https://relay.example/auth/refresh'),
      headers: {'Content-Type': 'application/json'},
      body: body,
    );
    expect(
      _proofVerifies(
        device,
        sent.single.headers[deviceProofHeader],
        purpose: proofPurposeRefresh,
        binding: 'POST https://relay.example/auth/refresh',
        token: 'bzr_refresh',
        body: utf8.encode(body),
      ),
      isTrue,
    );
  });

  test('other calls, and phones without a key, send no proof', () async {
    final http = client((_) => _ok());
    await http.get(
      Uri.parse('https://relay.example/media/abc'),
      headers: {'Authorization': 'Bearer bza_token'},
    );
    await http.post(
      Uri.parse('https://relay.example/auth/oidc/complete'),
      body: '{}',
    );
    device.failSigning = true;
    await http.get(
      Uri.parse('https://relay.example/auth/me'),
      headers: {'Authorization': 'Bearer bza_token'},
    );
    expect(sent.map((r) => r.headers[deviceProofHeader]), [null, null, null]);
  });

  test('a proof refused for a skewed clock is re-signed once', () async {
    final relayNow = DateTime.now().millisecondsSinceEpoch ~/ 1000 + 600;
    final http = client(
      (request) => _timed(sent.length == 1 ? 401 : 200, relayNow),
    );
    final response = await http.get(
      Uri.parse('https://relay.example/auth/me'),
      headers: {'Authorization': 'Bearer bza_token'},
    );
    expect(response.statusCode, 200);
    expect(sent, hasLength(2));
    final ts = int.parse(sent[1].headers[deviceProofHeader]!.split('.').first);
    expect((ts - relayNow).abs(), lessThan(5));
  });

  test('every /auth answer teaches its relay clock', () async {
    final relayNow = DateTime.now().millisecondsSinceEpoch ~/ 1000 + 600;
    final http = client((_) => _timed(401, relayNow));
    // No bearer: passes through unproven, but the clock is still learned.
    await http.get(Uri.parse('https://relay.example/auth/me'));
    expect(sent.single.headers[deviceProofHeader], isNull);
    expect(hasServerClock('https://relay.example'), isTrue);
    expect(hasServerClock('https://other.example'), isFalse);
    expect((proofNow('https://relay.example') - relayNow).abs(), lessThan(5));
  });

  group('refresh', () {
    test('a refresh this phone cannot sign is not sent', () async {
      final http = client((_) => _ok());
      device.failSigning = true;
      await expectLater(
        http.post(
          Uri.parse('https://relay.example/auth/refresh'),
          headers: {'Content-Type': 'application/json'},
          body: jsonEncode({'refresh': 'bzr_refresh'}),
        ),
        throwsA(isA<DeviceProofUnavailableException>()),
      );
      expect(sent, isEmpty);

      // Through the session API it is a transient failure: the session stays.
      final api = AuthApi(origin: 'https://relay.example', client: http);
      await expectLater(
        api.refresh('bzr_refresh'),
        throwsA(
          isA<AuthApiException>().having(
            (e) => e.kind,
            'kind',
            AuthFailureKind.transient,
          ),
        ),
      );
      expect(sent, isEmpty);
    });

    test('a refused refresh proof keeps the session', () async {
      for (final code in ['device_proof_required', 'device_proof_stale']) {
        final api = AuthApi(
          origin: 'https://relay.example',
          client: http_testing.MockClient(
            (_) async => http.Response(jsonEncode({'code': code}), 401),
          ),
        );
        await expectLater(
          api.refresh('bzr_refresh'),
          throwsA(
            isA<AuthApiException>().having(
              (e) => e.kind,
              'kind',
              AuthFailureKind.transient,
            ),
          ),
        );
      }
      // A revoked refresh still ends the session.
      final api = AuthApi(
        origin: 'https://relay.example',
        client: http_testing.MockClient(
          (_) async =>
              http.Response(jsonEncode({'code': 'invalid_grant'}), 401),
        ),
      );
      await expectLater(
        api.refresh('bzr_refresh'),
        throwsA(isA<AuthApiException>().having((e) => e.isTerminal, 't', true)),
      );
    });
  });

  group('sign-in', () {
    test('a fresh install proves the login in relay time', () async {
      final relayNow = DateTime.now().millisecondsSinceEpoch ~/ 1000 + 600;
      String? proof;
      final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
      final api = AuthApi(
        origin: 'https://relay.example',
        client: http_testing.MockClient((request) async {
          if (request.url.path == '/auth/me') return _timed(401, relayNow);
          proof =
              (jsonDecode(request.body) as Map<String, Object?>)['device_proof']
                  as String?;
          return _grant(relayNow);
        }),
        deviceKey: device,
      );
      await runOidcLogin(api: api, launcher: launcher);
      expect(launcher.opened, hasLength(1));
      final ts = int.parse(proof!.split('.').first);
      expect((ts - relayNow).abs(), lessThan(5));
      expect(
        _proofVerifies(
          device,
          proof,
          purpose: proofPurposeBind,
          binding: loginProofBinding('bzl_code'),
        ),
        isTrue,
      );
    });

    test('a stale login proof signs in again once, in relay time', () async {
      final relayNow = DateTime.now().millisecondsSinceEpoch ~/ 1000 + 600;
      final proofs = <String>[];
      final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
      final api = AuthApi(
        origin: 'https://relay.example',
        client: http_testing.MockClient((request) async {
          // The clock probe fails: the first proof goes out in phone time.
          if (request.url.path == '/auth/me') throw Exception('offline');
          final proof =
              (jsonDecode(request.body) as Map<String, Object?>)['device_proof']
                  as String;
          proofs.add(proof);
          final ts = int.parse(proof.split('.').first);
          if ((ts - relayNow).abs() > 60) {
            return http.Response(
              jsonEncode({'code': 'device_proof_stale'}),
              401,
              headers: {serverTimeHeader: '$relayNow'},
            );
          }
          return _grant(relayNow);
        }),
        deviceKey: device,
      );
      final grant = await runOidcLogin(api: api, launcher: launcher);
      expect(grant.accessToken, 'a');
      expect(launcher.opened, hasLength(2));
      expect(proofs, hasLength(2));

      // Still stale after the restart: the error surfaces, no third try.
      final stuck = AuthApi(
        origin: 'https://stuck.example',
        client: http_testing.MockClient(
          (request) async =>
              http.Response(jsonEncode({'code': 'device_proof_stale'}), 401),
        ),
        deviceKey: device,
      );
      final again = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
      await expectLater(
        runOidcLogin(api: stuck, launcher: again),
        throwsA(isA<AuthApiException>()),
      );
      expect(again.opened, hasLength(2));
    });

    test('start binds and complete proves the device key', () async {
      Map<String, Object?>? completeBody;
      final api = AuthApi(
        origin: 'https://relay.example',
        client: http_testing.MockClient((request) async {
          completeBody = jsonDecode(request.body) as Map<String, Object?>;
          return http.Response(
            jsonEncode({
              'principal_id': 'p',
              'device_id': 'd',
              'access': 'a',
              'refresh': 'r',
              'expires_in': 3600,
            }),
            200,
          );
        }),
        deviceKey: device,
      );
      final info = await api.deviceKeyInfo();
      final start = api.startUri(
        provider: 'google',
        state: 's',
        codeChallenge: 'c',
        redirectUri: 'kuzz://auth/cb',
        devicePubkey: info!.publicKeyHex,
      );
      expect(start.queryParameters['device_pubkey'], info.publicKeyHex);

      await api.completeLogin(
        loginCode: 'bzl_code',
        codeVerifier: 'v',
        device: info,
      );
      final body = completeBody!;
      expect(body['device_pubkey'], info.publicKeyHex);
      expect(body['device_key_kind'], 'hardware');
      expect(
        _proofVerifies(
          device,
          body['device_proof'] as String?,
          purpose: proofPurposeBind,
          binding: loginProofBinding('bzl_code'),
        ),
        isTrue,
      );
    });

    test('without a device key the login is unchanged', () async {
      Map<String, Object?>? completeBody;
      final api = AuthApi(
        origin: 'https://relay.example',
        client: http_testing.MockClient((request) async {
          completeBody = jsonDecode(request.body) as Map<String, Object?>;
          return http.Response(
            jsonEncode({
              'principal_id': 'p',
              'device_id': 'd',
              'access': 'a',
              'refresh': 'r',
            }),
            200,
          );
        }),
      );
      expect(await api.deviceKeyInfo(), isNull);
      await api.completeLogin(loginCode: 'bzl_code', codeVerifier: 'v');
      expect(
        completeBody!.keys,
        unorderedEquals(['login_code', 'code_verifier']),
      );
    });
  });
}

http.Response _ok() => http.Response('{}', 200);

http.Response _grant(int serverTime) => http.Response(
  jsonEncode({
    'principal_id': 'p',
    'device_id': 'd',
    'access': 'a',
    'refresh': 'r',
    'expires_in': 3600,
  }),
  200,
  headers: {serverTimeHeader: '$serverTime'},
);

http.Response _timed(int status, int serverTime) => http.Response(
  '{"code":"device_proof_stale"}',
  status,
  headers: {serverTimeHeader: '$serverTime'},
);
