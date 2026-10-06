import 'dart:async';

import 'package:buzz/shared/auth/token/mobile_callback.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:package_info_plus/package_info_plus.dart';

import 'token_auth_test_fakes.dart';
import 'token_session_test.dart' show Harness;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  for (final address in [
    'other.app://auth/cb',
    'xyz.block.buzz://other/cb',
    'xyz.block.buzz://auth/other',
    'xyz.block.buzz://user@auth/cb',
    'xyz.block.buzz://auth:123/cb',
    'xyz.block.buzz://auth/cb#fragment',
  ]) {
    test(
      'rejects callback address $address before exchanging a code',
      () async {
        final server = FakeAuthServer();
        final launcher = FakeWebAuthLauncher(
          (start) => Uri.parse(address).replace(
            queryParameters: {
              'state': start.queryParameters['state']!,
              'code': 'bzl_test',
            },
          ),
        );
        await expectLater(
          runOidcLogin(
            api: AuthApi(
              origin: 'https://relay.example',
              client: server.client,
            ),
            launcher: launcher,
          ),
          throwsA(
            isA<OidcLoginException>().having(
              (error) => error.code,
              'code',
              'invalid_callback',
            ),
          ),
        );
        expect(server.requests, isEmpty);
      },
    );
  }

  for (final parameter in ['state', 'code', 'error']) {
    test('rejects duplicate $parameter before exchanging a code', () async {
      final server = FakeAuthServer();
      final launcher = FakeWebAuthLauncher((start) {
        final good = FakeWebAuthLauncher.success(start);
        return Uri.parse('$good&$parameter=one&$parameter=two');
      });
      await expectLater(
        runOidcLogin(
          api: AuthApi(origin: 'https://relay.example', client: server.client),
          launcher: launcher,
        ),
        throwsA(
          isA<OidcLoginException>().having(
            (error) => error.code,
            'code',
            'invalid_callback',
          ),
        ),
      );
      expect(server.requests, isEmpty);
    });
  }

  test(
    'installed Android identity selects the callback in the real login',
    () async {
      PackageInfo.setMockInitialValues(
        appName: 'Buzz test',
        packageName: 'xyz.block.buzz.mobile.parity_dev',
        version: '1',
        buildNumber: '1',
        buildSignature: '',
      );
      final scheme = await const FlutterWebAuth2Launcher().callbackScheme();
      expect(scheme, 'xyz.block.buzz.parity-dev');
      final h = Harness();
      addTearDown(h.controller.dispose);
      h.launcher.installedScheme = scheme;
      await h.controller.restore();
      expect(await h.controller.signIn(), isTrue);
      expect(h.launcher.schemes, [scheme]);
      expect(
        h.launcher.opened.single.queryParameters['redirect_uri'],
        '$scheme://auth/cb',
      );
      expect(h.controller.currentAccessToken, 'bzs_login');
    },
  );

  test(
    'production scheme stays stable and invalid app identities fail closed',
    () {
      expect(
        callbackSchemeForAndroidPackage('xyz.block.buzz.mobile'),
        buzzMobileCallbackScheme,
      );
      for (final name in [
        'another.app',
        'xyz.block.buzz.mobile.',
        'xyz.block.buzz.mobile.bad/suffix',
      ]) {
        expect(() => callbackSchemeForAndroidPackage(name), throwsStateError);
      }
    },
  );

  const channel = MethodChannel('flutter_web_auth_2');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));

  test('two launchers cannot overwrite the pending platform callback', () async {
    final firstCallback = Completer<String>();
    var opened = 0;
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method != 'authenticate') return null;
      opened++;
      return firstCallback.future;
    });
    final first = const FlutterWebAuth2Launcher().authenticate(
      url: Uri.parse('https://a.example/auth/start'),
      callbackUrlScheme: buzzMobileCallbackScheme,
    );
    await expectLater(
      const FlutterWebAuth2Launcher().authenticate(
        url: Uri.parse('https://b.example/auth/start'),
        callbackUrlScheme: buzzMobileCallbackScheme,
      ),
      throwsA(isA<WebAuthBusyException>()),
    );
    expect(opened, 1);
    firstCallback.complete('xyz.block.buzz://auth/cb?code=first');
    expect((await first).queryParameters['code'], 'first');
    // Completion releases the browser for the next explicitly initiated login.
    await const FlutterWebAuth2Launcher().authenticate(
      url: Uri.parse('https://b.example/auth/start'),
      callbackUrlScheme: buzzMobileCallbackScheme,
    );
    expect(opened, 2);
  });

  test('platform cancellation releases browser ownership for retry', () async {
    var cancelled = true;
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method != 'authenticate') return null;
      if (cancelled) throw PlatformException(code: 'CANCELED');
      return buzzMobileRedirectUri;
    });
    Future<Uri> launch() => const FlutterWebAuth2Launcher().authenticate(
      url: Uri.parse('https://relay.example/auth/start'),
      callbackUrlScheme: buzzMobileCallbackScheme,
    );
    await expectLater(launch(), throwsA(isA<WebAuthCancelledException>()));
    cancelled = false;
    expect(await launch(), Uri.parse(buzzMobileRedirectUri));
  });

  for (final failure in <Object>[
    const WebAuthCancelledException(),
    const WebAuthBusyException(),
    const OidcLoginException('Rejected callback'),
    const AuthApiException(AuthFailureKind.transient, 'Failed exchange'),
    StateError('Browser unavailable'),
  ]) {
    test(
      'late ${failure.runtimeType} cannot overwrite newer recovery state',
      () async {
        final h = Harness();
        addTearDown(h.controller.dispose);
        await h.controller.restore();
        final browser = h.launcher.gate = Completer<void>();
        h.launcher.error = failure;
        final pending = h.controller.signIn();
        await Future<void>.delayed(Duration.zero);
        await h.controller.signOut();
        h.store.readError = StateError('Keychain unavailable');
        await h.controller.restore();
        final settled = h.controller.state;
        expect(settled.status, TokenSessionStatus.stalled);
        final notifications = h.states.length;
        browser.complete();
        expect(await pending, isFalse);
        expect(h.controller.state, settled);
        expect(h.states.length, notifications);
        expect(h.store.writes, 0);
      },
    );
  }

  test('duplicate taps share one login; cancelled retry can succeed', () async {
    final h = Harness();
    addTearDown(h.controller.dispose);
    await h.controller.restore();
    final browser = h.launcher.gate = Completer<void>();
    h.launcher.error = const WebAuthCancelledException();
    final first = h.controller.signIn();
    final second = h.controller.signIn();
    expect(identical(first, second), isTrue);
    await Future<void>.delayed(Duration.zero);
    expect(h.launcher.opened, hasLength(1));
    browser.complete();
    expect(await first, isFalse);
    expect(await second, isFalse);
    h.launcher.error = null;
    expect(await h.controller.signIn(), isTrue);
    expect(h.launcher.opened, hasLength(2));
  });

  test('late rejected-mode cleanup cannot overwrite newer recovery', () async {
    final h = Harness();
    addTearDown(h.controller.dispose);
    await h.controller.restore();
    final cleanup = h.server.heldLogout = Completer<http.Response>();
    // The default grant is token mode; key backup must reject it.
    final pending = h.controller.signIn(identityMode: 'key_backup');
    await settle();
    expect(h.logouts, hasLength(1));
    await h.controller.forgetOnDevice();
    h.store.readError = StateError('Keychain unavailable');
    await h.controller.restore();
    final settled = h.controller.state;
    final notifications = h.states.length;
    cleanup.complete(http.Response('', 204));
    expect(await pending, isFalse);
    expect(h.controller.state, settled);
    expect(h.states.length, notifications);
    expect(h.store.writes, 0);
  });
}
