import 'dart:async';

import 'package:buzz/features/settings/experiments_settings.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/read_aloud/speech_audio_gate.dart';
import 'package:buzz/shared/read_aloud/read_aloud_controller.dart';
import 'package:buzz/shared/read_aloud/read_aloud_message.dart';
import 'package:buzz/shared/read_aloud/read_aloud_preferences.dart';
import 'package:buzz/shared/read_aloud/speech_engine.dart';
import 'package:buzz/shared/read_aloud/spoken_text_surface.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

class FakeSpeech implements SpeechEngine {
  final chunks = <String>[];
  final callbacks = <void Function(int, int)>[];
  Completer<bool>? pending;
  Completer<void>? preparation;
  Object? failure;
  int stops = 0;
  @override
  Future<void> prepare(String language) async {
    if (failure != null) throw failure!;
    await preparation?.future;
  }

  @override
  Future<bool> speak(String text, void Function(int, int) onRange) {
    chunks.add(text);
    callbacks.add(onRange);
    pending = Completer<bool>();
    return pending!.future;
  }

  void finish() {
    pending?.complete(true);
    pending = null;
  }

  @override
  Future<void> stop() async {
    stops++;
    pending?.complete(false);
    pending = null;
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late SharedPreferences prefs;
  late FakeSpeech engine;
  late ProviderContainer container;

  setUp(() async {
    SharedPreferences.setMockInitialValues({ReadAloudPreference.key: true});
    prefs = await SharedPreferences.getInstance();
    engine = FakeSpeech();
    container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        speechEngineProvider.overrideWithValue(engine),
      ],
    );
  });
  tearDown(() => container.dispose());

  test(
    'pause resumes at native word offset, stop resets, stale ranges ignored',
    () async {
      final controller = container.read(readAloudControllerProvider.notifier);
      final owner = Object();
      await controller.play(owner, '하나 둘 셋', 'ko');
      engine.callbacks.last(3, 4);
      await controller.pause();
      expect(container.read(readAloudControllerProvider).start, 3);
      await controller.resume();
      expect(engine.chunks.last, '둘 셋');
      engine.callbacks.first(0, 2);
      expect(container.read(readAloudControllerProvider).start, 3);
      engine.callbacks.last(2, 3);
      expect(container.read(readAloudControllerProvider).start, 5);
      await controller.stop();
      expect(
        container.read(readAloudControllerProvider).phase,
        ReadAloudPhase.idle,
      );
      await controller.play(owner, '하나 둘 셋', 'ko');
      expect(engine.chunks.last, '하나 둘 셋');
    },
  );

  test(
    'switching messages ignores old callbacks and old widget cleanup',
    () async {
      final controller = container.read(readAloudControllerProvider.notifier);
      final first = Object(), second = Object();
      await controller.play(first, '첫 번째', 'ko');
      await controller.play(second, '두 번째', 'ko');
      engine.callbacks.first(1, 2);
      await controller.stopOwner(first);
      expect(container.read(readAloudControllerProvider).owner, same(second));
      expect(container.read(readAloudControllerProvider).start, 0);
    },
  );

  test('stop during voice initialization cannot start late speech', () async {
    final controller = container.read(readAloudControllerProvider.notifier);
    engine.preparation = Completer<void>();
    final starting = controller.play(Object(), '취소할 메시지', 'ko');
    await Future<void>.delayed(Duration.zero);
    final stopping = controller.stop();
    engine.preparation!.complete();
    await Future.wait([starting, stopping]);
    expect(engine.chunks, isEmpty);
    expect(container.read(readAloudControllerProvider).active, false);
  });

  test('long messages complete all chunks without truncation', () async {
    final controller = container.read(readAloudControllerProvider.notifier);
    final text = List.filled(1500, '안녕 😀 ').join();
    await controller.play(Object(), text, 'ko');
    while (container.read(readAloudControllerProvider).active) {
      engine.finish();
      await Future<void>.delayed(Duration.zero);
    }
    expect(engine.chunks.join(), text);
    expect(engine.chunks.every((chunk) => chunk.length <= 600), true);
    expect(speechChunkEnd('${'a' * 599}😀끝', 0), 599);
  });

  test(
    'changing engine persists locally and stops the old speech session',
    () async {
      final controller = container.read(readAloudControllerProvider.notifier);
      await controller.play(Object(), '본문', 'ko');
      await container
          .read(readAloudEngineProvider.notifier)
          .setEngine('com.google.android.tts');
      await Future<void>.delayed(Duration.zero);
      expect(container.read(readAloudControllerProvider).active, false);
      expect(
        prefs.getString(ReadAloudEnginePreference.key),
        'com.google.android.tts',
      );
      container.invalidate(readAloudEngineProvider);
      expect(container.read(readAloudEngineProvider), 'com.google.android.tts');
    },
  );

  test(
    'preference disable, community transition and huddle stop speech',
    () async {
      final controller = container.read(readAloudControllerProvider.notifier);
      await controller.play(Object(), '본문', 'ko');
      await container.read(readAloudEnabledProvider.notifier).setEnabled(false);
      expect(container.read(readAloudControllerProvider).active, false);
      await container.read(readAloudEnabledProvider.notifier).setEnabled(true);
      await controller.play(Object(), '본문', 'ko');
      await container.read(communityTransitionProvider).run();
      expect(container.read(readAloudControllerProvider).active, false);
      await controller.play(Object(), '본문', 'ko');
      await container.read(speechAudioGateProvider).acquire(Object());
      expect(container.read(readAloudControllerProvider).active, false);
      await controller.play(Object(), '본문', 'ko');
      expect(
        container.read(readAloudControllerProvider).phase,
        ReadAloudPhase.failed,
      );
    },
  );

  test(
    'missing offline voice produces actionable error and remains retryable',
    () async {
      final controller = container.read(readAloudControllerProvider.notifier);
      engine.failure = const SpeechFailure('한국어 음성을 설치해 주세요.');
      await controller.play(Object(), '본문', 'ko');
      expect(container.read(readAloudControllerProvider).error, contains('설치'));
      engine.failure = null;
      await controller.play(Object(), '본문', 'ko');
      expect(
        container.read(readAloudControllerProvider).phase,
        ReadAloudPhase.speaking,
      );
    },
  );

  test('voice selection rejects network, unknown and missing voices', () {
    final voices = [
      {'name': 'online', 'locale': 'ko-KR', 'network_required': '1'},
      {'name': 'unknown', 'locale': 'ko-KR'},
      {
        'name': 'missing',
        'locale': 'ko-KR',
        'network_required': '0',
        'features': 'notInstalled',
      },
      {'name': 'local', 'locale': 'ko-KR', 'network_required': '0'},
    ];
    expect(offlineSpeechVoice(voices, 'ko', android: true)?['name'], 'local');
    expect(offlineSpeechVoice(voices, 'fr', android: true), null);
  });

  testWidgets(
    'speech underline stays inside horizontally scrolled code viewport',
    (tester) async {
      final key = GlobalKey();
      final scroll = ScrollController();
      addTearDown(scroll.dispose);
      Widget surface(String? text) => MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 100,
            child: SpokenTextSurface(
              key: key,
              spokenText: text,
              start: 0,
              end: text?.length ?? 0,
              color: Colors.blue,
              child: SingleChildScrollView(
                controller: scroll,
                scrollDirection: Axis.horizontal,
                child: const Text(
                  'a very long line of code that does not wrap',
                  softWrap: false,
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpWidget(surface(null));
      final box =
          key.currentContext!.findRenderObject()! as SpokenTextRenderBox;
      await tester.pumpWidget(surface(box.captureText()));
      scroll.jumpTo(80);
      await tester.pump();
      final rectangles = box.highlightRects().where((rect) => !rect.isEmpty);
      expect(rectangles, isNotEmpty);
      expect(
        rectangles.every((rect) => rect.left >= 0 && rect.right <= 100),
        true,
      );
    },
  );

  testWidgets(
    'settings default off, persist toggle and show per-message controls',
    (tester) async {
      await prefs.remove(ReadAloudPreference.key);
      container.invalidate(readAloudEnabledProvider);
      await tester.pumpWidget(
        UncontrolledProviderScope(
          container: container,
          child: MaterialApp(
            theme: AppTheme.light(),
            home: Scaffold(
              body: Column(
                children: [
                  const ExperimentsSettings(),
                  ReadAloudMessage(
                    messageId: 'one',
                    content: '하나 둘',
                    child: const Text('하나 둘'),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
      expect(find.byTooltip('메시지 읽어주기'), findsNothing);
      await tester.tap(find.byKey(const ValueKey('read-aloud-enabled')));
      await tester.pumpAndSettle();
      expect(prefs.getBool(ReadAloudPreference.key), true);
      expect(find.byTooltip('메시지 읽어주기'), findsOneWidget);
      await tester.tap(find.byTooltip('메시지 읽어주기'));
      await tester.pump();
      expect(engine.chunks.single, '하나 둘\n');
      engine.callbacks.last(3, 4);
      await tester.pump();
      expect(find.byTooltip('읽어주기 중지'), findsOneWidget);
      await tester.tap(find.byTooltip('일시정지'));
      await tester.pump();
      expect(find.byTooltip('이어 읽기'), findsOneWidget);
      await tester.tap(find.byTooltip('이어 읽기'));
      await tester.pump();
      expect(engine.chunks.last, '둘\n');
      await tester.tap(find.byTooltip('읽어주기 중지'));
      await tester.pump();
      expect(find.byTooltip('메시지 읽어주기'), findsOneWidget);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pump();
    },
  );

  testWidgets(
    'rendered inline text ordering and multiline highlight preserve layout',
    (tester) async {
      final key = GlobalKey();
      Widget surface(String? text, int start, int end) => MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 110,
            child: SpokenTextSurface(
              key: key,
              spokenText: text,
              start: start,
              end: end,
              color: Colors.blue,
              child: const Text.rich(
                TextSpan(
                  children: [
                    TextSpan(text: '앞 '),
                    WidgetSpan(child: Text('링크')),
                    TextSpan(text: ' 뒤\n긴 문장을 여러 줄에 표시합니다.'),
                  ],
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpWidget(surface(null, 0, 0));
      final box =
          key.currentContext!.findRenderObject()! as SpokenTextRenderBox;
      final text = box.captureText();
      expect(text, '앞 링크 뒤\n긴 문장을 여러 줄에 표시합니다.\n');
      final size = box.size;
      await tester.pumpWidget(surface(text, 0, text.length));
      expect(box.highlightRects().length, greaterThan(2));
      expect(box.size, size);
      await tester.pumpWidget(surface('stale text', 0, 3));
      expect(box.highlightRects(), isEmpty);
    },
  );
}
