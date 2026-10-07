import 'dart:async';

import 'package:buzz/shared/read_aloud/speech_audio_session.dart';
import 'package:buzz/shared/read_aloud/speech_engine.dart';
import 'package:buzz/shared/read_aloud/system_speech_engine.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

class FakeAudio implements SpeechAudioSession {
  final calls = <String>[];
  void Function()? interrupt;
  @override
  Future<void> activate(void Function() interrupted) async {
    calls.add('activate');
    interrupt = interrupted;
  }

  @override
  Future<void> release() async {
    calls.add('release');
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('flutter_tts');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  Future<void> emit(String method, [dynamic arguments]) async {
    final done = Completer<void>();
    messenger.handlePlatformMessage(
      channel.name,
      const StandardMethodCodec().encodeMethodCall(
        MethodCall(method, arguments),
      ),
      (_) => done.complete(),
    );
    await done.future;
  }

  late FakeAudio audio;
  late SystemSpeechEngine engine;
  late List<String> calls;
  setUp(() {
    audio = FakeAudio();
    calls = [];
    engine = SystemSpeechEngine(
      audio: audio,
      android: true,
      setupTimeout: const Duration(milliseconds: 30),
      speechTimeout: const Duration(seconds: 1),
    );
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call.method);
      if (call.method == 'getVoices') {
        return [
          {'name': 'ko-local', 'locale': 'ko-KR', 'network_required': '0'},
        ];
      }
      if (call.method == 'stop') await emit('speak.onCancel');
      return 1;
    });
  });
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));

  test(
    'stop drains cancellation and releases focus before next speech',
    () async {
      await engine.prepare('ko');
      final first = engine.speak('하나 둘', (_, _) {});
      await Future<void>.delayed(Duration.zero);
      await engine.stop();
      expect(await first, false);
      expect(audio.calls, ['activate', 'release']);
      final second = engine.speak('셋 넷', (_, _) {});
      await Future<void>.delayed(Duration.zero);
      await emit('speak.onComplete');
      expect(await second, true);
      expect(audio.calls, ['activate', 'release', 'activate', 'release']);
    },
  );

  test(
    'native start exception settles immediately without later timer error',
    () async {
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'speak') {
          throw PlatformException(code: 'start-failed');
        }
        return 1;
      });
      await expectLater(
        engine.speak('안녕', (_, _) {}),
        throwsA(isA<PlatformException>()),
      );
      expect(audio.calls.last, 'release');
      await Future<void>.delayed(const Duration(milliseconds: 1100));
    },
  );

  test(
    'native error while start method is pending is observed immediately',
    () async {
      final start = Completer<int>();
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'speak') return start.future;
        return 1;
      });
      final speaking = engine.speak('안녕', (_, _) {});
      final checked = expectLater(speaking, throwsA(isA<SpeechFailure>()));
      await Future<void>.delayed(Duration.zero);
      await emit('speak.onError', 'native error');
      await checked;
      start.complete(1);
      expect(audio.calls.last, 'release');
    },
  );

  test(
    'late voice enumeration after timeout cannot configure another session',
    () async {
      final voices = Completer<List<Map<String, String>>>();
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call.method);
        if (call.method == 'getVoices') return voices.future;
        return 1;
      });
      await expectLater(engine.prepare('ko'), throwsA(isA<TimeoutException>()));
      await expectLater(engine.prepare('ko'), throwsA(isA<SpeechFailure>()));
      voices.complete([
        {'name': 'ko', 'locale': 'ko-KR', 'network_required': '0'},
      ]);
      await Future<void>.delayed(Duration.zero);
      expect(calls.where((call) => call == 'setVoice'), isEmpty);
      expect(audio.calls, isEmpty);
    },
  );

  test(
    'Android whole-utterance start event is not shown as word progress',
    () async {
      final ranges = <(int, int)>[];
      final speaking = engine.speak(
        '하나 둘',
        (start, end) => ranges.add((start, end)),
      );
      await Future<void>.delayed(Duration.zero);
      await emit('speak.onProgress', {
        'text': '하나 둘',
        'start': '0',
        'end': '4',
        'word': '하나 둘',
      });
      expect(ranges, isEmpty);
      await emit('speak.onProgress', {
        'text': '하나 둘',
        'start': '0',
        'end': '2',
        'word': '하나',
      });
      expect(ranges, [(0, 2)]);
      await engine.stop();
      await speaking;
    },
  );
}
