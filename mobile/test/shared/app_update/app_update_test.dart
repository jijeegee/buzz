import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:kuzz/shared/app_update/app_update.dart';
import 'package:kuzz/shared/app_update/app_update_gate.dart';
import 'package:kuzz/shared/relay/relay_socket.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

class _FakeUpdate extends AppUpdateNotifier {
  _FakeUpdate(this.initial);

  final AppUpdateState initial;
  int installs = 0;

  @override
  AppUpdateState build() => initial;

  @override
  Future<void> install() async => installs++;
}

const _release = AppRelease(
  version: '0.6.2',
  url: 'https://example.invalid/kuzz.apk',
  updateAvailable: true,
  required: false,
);

Future<_FakeUpdate> _pump(WidgetTester tester, AppUpdateState state) async {
  final fake = _FakeUpdate(state);
  await tester.pumpWidget(
    ProviderScope(
      overrides: [appUpdateProvider.overrideWith(() => fake)],
      child: const MaterialApp(
        home: AppUpdateGate(child: Scaffold(body: Text('app content'))),
      ),
    ),
  );
  return fake;
}

void main() {
  test('reads the relay answer', () {
    final release = AppRelease.fromJson({
      'version': '0.6.2',
      'url': 'https://example.invalid/kuzz.apk',
      'signature': '',
      'min_version': '0.6.2',
      'update_available': true,
      'required': true,
    });
    expect(release.version, '0.6.2');
    expect(release.updateAvailable, isTrue);
    expect(release.required, isTrue);
  });

  test('only a 426 upgrade refusal means update required', () {
    const refused = WebSocketException('not upgraded to websocket', 426);
    expect(isUpdateRequiredRefusal(refused), isTrue);
    expect(
      isUpdateRequiredRefusal(WebSocketChannelException.from(refused)),
      isTrue,
    );
    expect(
      isUpdateRequiredRefusal(
        WebSocketChannelException.from(
          const WebSocketException('not upgraded to websocket', 503),
        ),
      ),
      isFalse,
    );
    expect(isUpdateRequiredRefusal(StateError('426')), isFalse);
  });

  test(
    'auth calls name the build and a 426 raises the update signal',
    () async {
      final seen = <String, String?>{};
      final client = AppClientHeaderClient(
        MockClient((request) async {
          seen[request.url.path] = request.headers[appClientHeader];
          return request.url.path == '/auth/refresh'
              ? http.Response('{"code":"update_required"}', 426)
              : http.Response('{}', 200);
        }),
        clientId: () async => 'android/0.6.2',
      );
      final before = updateRequiredSignal.value;
      final ok = await client.get(Uri.parse('https://relay.example/auth/me'));
      expect(ok.statusCode, 200);
      expect(updateRequiredSignal.value, before);
      await client.get(Uri.parse('https://relay.example/'));
      final refused = await client.post(
        Uri.parse('https://relay.example/auth/refresh'),
      );
      expect(refused.statusCode, 426, reason: 'the caller still sees the 426');
      expect(updateRequiredSignal.value, before + 1);
      expect(seen['/auth/me'], 'android/0.6.2');
      expect(seen['/auth/refresh'], 'android/0.6.2');
      expect(seen['/'], isNull, reason: 'only account calls are named');
    },
  );

  test('a relay refusing the build raises the update signal', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    server.listen((request) {
      request.response
        ..statusCode = 426
        ..write('update-required');
      request.response.close();
    });
    final before = updateRequiredSignal.value;
    Object? disconnectError;
    final socket = RelaySocket(
      wsUrl: 'ws://127.0.0.1:${server.port}',
      nsec: null,
      onMessage: (_) {},
      onConnected: () {},
      onDisconnected: (error) => disconnectError = error,
    );
    addTearDown(socket.dispose);

    await socket.connect();

    expect(updateRequiredSignal.value, before + 1);
    expect(isUpdateRequiredRefusal(disconnectError!), isTrue);
    expect(socket.state, SocketState.disconnected);
  });

  testWidgets('no update leaves the app alone', (tester) async {
    await _pump(tester, const AppUpdateState());
    expect(find.text('app content'), findsOneWidget);
    expect(find.text('Update'), findsNothing);
  });

  testWidgets('optional update offers Update and Later', (tester) async {
    final fake = await _pump(tester, const AppUpdateState(release: _release));
    expect(find.text('app content'), findsOneWidget);
    expect(find.text('Kuzz 0.6.2 is available.'), findsOneWidget);

    await tester.tap(find.text('Update'));
    expect(fake.installs, 1);

    await tester.tap(find.text('Later'));
    await tester.pump();
    expect(find.text('Kuzz 0.6.2 is available.'), findsNothing);
    expect(find.text('app content'), findsOneWidget);
  });

  testWidgets('required update hides the app and cannot be dismissed', (
    tester,
  ) async {
    final fake = await _pump(
      tester,
      const AppUpdateState(release: _release, forced: true),
    );
    expect(find.text('app content'), findsNothing);
    expect(find.text('Update required'), findsOneWidget);
    expect(find.text('Later'), findsNothing);

    await tester.tap(find.text('Update'));
    expect(fake.installs, 1);
  });

  testWidgets('refused before a release is known offers a re-check', (
    tester,
  ) async {
    await _pump(tester, const AppUpdateState(forced: true));
    expect(find.text('Update required'), findsOneWidget);
    expect(find.text('Try again'), findsOneWidget);
  });
}
