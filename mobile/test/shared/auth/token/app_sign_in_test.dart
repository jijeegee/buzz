import 'dart:convert';
import 'dart:math';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:kuzz/shared/auth/token/token.dart';

import 'token_auth_test_fakes.dart';

const origin = 'https://relay.example';

class FakeGoogleAccountPicker implements GoogleAccountPicker {
  FakeGoogleAccountPicker(this.respond);

  final Future<String> Function(String clientId, String nonce) respond;
  final List<(String, String)> calls = [];

  @override
  Future<String> idToken({required String clientId, required String nonce}) {
    calls.add((clientId, nonce));
    return respond(clientId, nonce);
  }
}

/// A relay offering app sign-in; other paths go to [FakeAuthServer].
class AppSignInServer {
  final server = FakeAuthServer();
  final List<http.Request> requests = [];
  http.Response Function(http.Request) start = (_) =>
      jsonResponse({'nonce': 'n-1', 'client_id': 'web-client'});
  http.Response Function(http.Request) idToken = (_) =>
      jsonResponse({'login_code': 'bzl_app'});

  late final http.Client client = MockClient((request) async {
    requests.add(request);
    final path = request.url.path;
    if (path == '/auth/oidc/google/start' &&
        request.url.queryParameters['response'] == 'json') {
      return start(request);
    }
    if (path == '/auth/oidc/google/id-token') return idToken(request);
    final copy = http.Request(request.method, request.url)
      ..headers.addAll(request.headers)
      ..body = request.body;
    return http.Response.fromStream(await server.client.send(copy));
  });
}

void main() {
  test('signs in with the phone account, no browser', () async {
    final relay = AppSignInServer();
    final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
    final picker = FakeGoogleAccountPicker((_, _) async => 'id.token');
    final grant = await runOidcLogin(
      api: AuthApi(origin: origin, client: relay.client),
      launcher: launcher,
      accountPicker: picker,
      random: Random(3),
    );
    expect(grant.refreshToken, 'bzr_login');
    expect(launcher.opened, isEmpty);
    expect(picker.calls.single, ('web-client', 'n-1'));
    final start = relay.requests.first.url;
    expect(start.queryParameters['client'], 'mobile');
    final exchange = jsonDecode(relay.requests[1].body) as Map<String, dynamic>;
    expect(exchange, {
      'state': start.queryParameters['state'],
      'id_token': 'id.token',
    });
    final complete =
        jsonDecode(relay.server.requests.single.body) as Map<String, dynamic>;
    expect(complete['login_code'], 'bzl_app');
    expect(
      pkceChallenge(complete['code_verifier'] as String),
      start.queryParameters['code_challenge'],
    );
  });

  test('falls back to the browser when the phone has no picker', () async {
    final relay = AppSignInServer();
    final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
    final picker = FakeGoogleAccountPicker(
      (_, _) async =>
          throw const GoogleAccountUnavailableException('no_account'),
    );
    final grant = await runOidcLogin(
      api: AuthApi(origin: origin, client: relay.client),
      launcher: launcher,
      accountPicker: picker,
    );
    expect(grant.refreshToken, 'bzr_login');
    expect(launcher.opened.single.queryParameters['response'], isNull);
    // The browser login uses its own state, not the abandoned app one.
    expect(
      launcher.opened.single.queryParameters['state'],
      isNot(relay.requests.first.url.queryParameters['state']),
    );
  });

  test('falls back to the browser on a relay without app sign-in', () async {
    final relay = AppSignInServer()
      ..start = (_) => FakeAuthServer.authError(400, 'bad_request');
    final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
    final picker = FakeGoogleAccountPicker((_, _) async => 'unused');
    await runOidcLogin(
      api: AuthApi(origin: origin, client: relay.client),
      launcher: launcher,
      accountPicker: picker,
    );
    expect(picker.calls, isEmpty);
    expect(launcher.opened, hasLength(1));
  });

  test('closing the picker cancels without opening the browser', () async {
    final relay = AppSignInServer();
    final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
    final picker = FakeGoogleAccountPicker(
      (_, _) async => throw const WebAuthCancelledException(),
    );
    await expectLater(
      runOidcLogin(
        api: AuthApi(origin: origin, client: relay.client),
        launcher: launcher,
        accountPicker: picker,
      ),
      throwsA(isA<WebAuthCancelledException>()),
    );
    expect(launcher.opened, isEmpty);
  });

  test('a refused id_token is reported, not retried in the browser', () async {
    final relay = AppSignInServer()
      ..idToken = (_) => FakeAuthServer.authError(401, 'login_failed');
    final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
    final picker = FakeGoogleAccountPicker((_, _) async => 'id.token');
    await expectLater(
      runOidcLogin(
        api: AuthApi(origin: origin, client: relay.client),
        launcher: launcher,
        accountPicker: picker,
      ),
      throwsA(
        isA<AuthApiException>().having((e) => e.code, 'code', 'login_failed'),
      ),
    );
    expect(launcher.opened, isEmpty);
  });
}
