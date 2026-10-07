import 'dart:async';

import 'package:buzz/features/settings/read_aloud_engine_settings.dart';
import 'package:buzz/shared/read_aloud/read_aloud_preferences.dart';
import 'package:buzz/shared/read_aloud/system_speech_engine.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('flutter_tts');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  testWidgets(
    'picker saves the installed engine and playback reads that preference',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      final prefs = await SharedPreferences.getInstance();
      final calls = <MethodCall>[];
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call);
        if (call.method == 'getEngines') {
          return ['com.samsung.SMT', 'com.google.android.tts'];
        }
        if (call.method == 'getVoices') {
          return [
            {'name': 'ko-local', 'locale': 'ko-KR', 'network_required': '0'},
          ];
        }
        return 1;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      final container = ProviderContainer(
        overrides: [
          savedPrefsProvider.overrideWithValue(prefs),
          systemSpeechEngineProvider.overrideWith(
            (ref) => SystemSpeechEngine(
              android: true,
              selectedEngine: () => ref.read(readAloudEngineProvider),
            ),
          ),
        ],
      );
      addTearDown(container.dispose);
      await tester.pumpWidget(
        UncontrolledProviderScope(
          container: container,
          child: MaterialApp(
            theme: AppTheme.light(),
            home: const Scaffold(body: ReadAloudEngineSettings()),
          ),
        ),
      );
      await tester.tap(find.byKey(const ValueKey('read-aloud-engine')));
      await tester.pumpAndSettle();
      expect(find.text('삼성 TTS'), findsOneWidget);
      await tester.tap(find.text('Google 음성 인식 및 합성'));
      await tester.pumpAndSettle();
      expect(
        prefs.getString(ReadAloudEnginePreference.key),
        'com.google.android.tts',
      );
      container.invalidate(readAloudEngineProvider);
      await container.read(systemSpeechEngineProvider).prepare('ko');
      expect(
        calls.where((call) => call.method == 'setEngine').single.arguments,
        'com.google.android.tts',
      );
      // Reopen to discover engines again; cancel keeps the persisted choice.
      await tester.tap(find.byKey(const ValueKey('read-aloud-engine')));
      await tester.pumpAndSettle();
      Navigator.of(tester.element(find.byType(SimpleDialog))).pop();
      await tester.pumpAndSettle();
      expect(container.read(readAloudEngineProvider), 'com.google.android.tts');
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('discovery failure reports an error and the user can retry', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    var fail = true;
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (fail) throw PlatformException(code: 'unavailable');
      return ['com.samsung.SMT'];
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          savedPrefsProvider.overrideWithValue(prefs),
          systemSpeechEngineProvider.overrideWithValue(
            SystemSpeechEngine(android: true),
          ),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: const Scaffold(body: ReadAloudEngineSettings()),
        ),
      ),
    );
    await tester.tap(find.byKey(const ValueKey('read-aloud-engine')));
    await tester.pumpAndSettle();
    expect(find.textContaining('설치된 엔진 목록을 불러오지 못했습니다'), findsOneWidget);
    Navigator.of(tester.element(find.byType(SimpleDialog))).pop();
    await tester.pumpAndSettle();
    fail = false;
    await tester.tap(find.byKey(const ValueKey('read-aloud-engine')));
    await tester.pumpAndSettle();
    expect(find.text('삼성 TTS'), findsOneWidget);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets(
    'quarantined adapter still allows default reset and explains restart',
    (tester) async {
      SharedPreferences.setMockInitialValues({
        ReadAloudEnginePreference.key: 'com.samsung.SMT',
      });
      final prefs = await SharedPreferences.getInstance();
      final pending = Completer<List<dynamic>>();
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'getVoices') return pending.future;
        return 1;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      final engine = SystemSpeechEngine(
        android: true,
        setupTimeout: const Duration(milliseconds: 1),
      );
      await tester.runAsync(() async {
        await expectLater(
          engine.prepare('ko'),
          throwsA(isA<TimeoutException>()),
        );
      });
      pending.complete([]);
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            savedPrefsProvider.overrideWithValue(prefs),
            systemSpeechEngineProvider.overrideWithValue(engine),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: const Scaffold(body: ReadAloudEngineSettings()),
          ),
        ),
      );
      await tester.tap(find.byKey(const ValueKey('read-aloud-engine')));
      await tester.pumpAndSettle();
      expect(find.textContaining('앱을 다시 열어 주세요'), findsOneWidget);
      await tester.tap(find.text('휴대폰 기본 설정 따르기'));
      await tester.pumpAndSettle();
      expect(prefs.getString(ReadAloudEnginePreference.key), '');
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );
}
