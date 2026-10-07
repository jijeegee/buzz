import 'dart:async';
import 'dart:io';

import 'package:flutter_tts/flutter_tts.dart';

import 'speech_audio_session.dart';
import 'speech_engine.dart';

/// Free system TTS adapter. Speech text never goes to a Buzz/cloud endpoint.
class SystemSpeechEngine implements SpeechEngine {
  SystemSpeechEngine({
    SpeechAudioSession? audio,
    bool? android,
    this.setupTimeout = const Duration(seconds: 8),
    this.speechTimeout = const Duration(minutes: 2),
  }) : _audio = audio ?? DeviceSpeechAudioSession(),
       _android = android ?? Platform.isAndroid;

  final SpeechAudioSession _audio;
  final bool _android;
  final Duration setupTimeout;
  final Duration speechTimeout;
  FlutterTts? _tts;
  Completer<bool>? _utterance;
  Future<void>? _activation;
  Future<void>? _release;
  void Function(int, int)? _onRange;
  String? _text;
  bool _stopping = false;
  bool _unavailable = false;

  void _checkAvailable() {
    if (_unavailable) {
      throw const SpeechFailure('음성 엔진이 응답하지 않습니다. 앱을 다시 열어 주세요.');
    }
  }

  FlutterTts get _client {
    _checkAvailable();
    return _tts ??= FlutterTts()
      ..setCompletionHandler(() => _finish(true))
      ..setCancelHandler(() => _finish(false))
      ..setErrorHandler(
        (_) => _finish(
          false,
          error: const SpeechFailure(
            '읽어주기에 실패했습니다. 휴대폰의 음성 설정을 확인하고 다시 시도해 주세요.',
          ),
        ),
      )
      ..setProgressHandler((text, start, end, _) {
        if (!_stopping &&
            text == _text &&
            start >= 0 &&
            end > start &&
            end <= text.length) {
          // Android <26 reports the entire utterance at onStart. That is not
          // a word position. Keep the UI honest and resume this short phrase.
          if (_android && start == 0 && end == text.length) return;
          _onRange?.call(start, end);
        }
      });
  }

  void _finish(bool completed, {Object? error}) {
    final pending = _utterance;
    if (pending == null || pending.isCompleted) return;
    if (error == null) {
      pending.complete(completed);
    } else {
      pending.completeError(error);
    }
  }

  @override
  Future<void> prepare(String language) async {
    try {
      await _prepare(language).timeout(setupTimeout);
    } on TimeoutException {
      // Timeout doesn't cancel native futures. Quarantine this adapter so a
      // late voice result cannot configure a newer speech session.
      _unavailable = true;
      rethrow;
    }
  }

  Future<void> _prepare(String language) async {
    final client = _client;
    final voices = await client.getVoices;
    _checkAvailable();
    final voice = offlineSpeechVoice(
      voices is List ? voices : [],
      language,
      android: _android,
    );
    if (voice == null) {
      throw SpeechFailure(
        language == 'ko'
            ? '한국어 오프라인 음성이 없습니다. 휴대폰 설정의 텍스트 음성 변환에서 한국어 음성을 설치한 뒤 다시 눌러 주세요.'
            : '이 언어의 오프라인 음성이 없습니다. 휴대폰 음성 설정에서 음성을 설치한 뒤 다시 눌러 주세요.',
      );
    }
    if (await client.setVoice(voice) != 1) {
      throw const SpeechFailure('선택한 음성을 사용할 수 없습니다. 휴대폰 음성 설정을 확인해 주세요.');
    }
    _checkAvailable();
    await client.setSpeechRate(0.5);
    _checkAvailable();
    if (Platform.isIOS) {
      // Do not call setSharedInstance: it activates AVAudioSession before our
      // audio owner exists. Only SpeechAudioSession may activate/release it.
      await client.autoStopSharedSession(false);
    }
  }

  Future<void> _releaseAudio() => _release ??= _audio.release();

  @override
  Future<bool> speak(String text, void Function(int, int) onRange) async {
    final client = _client;
    if (_utterance != null) throw StateError('Speech must be serialized');
    final done = Completer<bool>();
    _utterance = done;
    _text = text;
    _onRange = onRange;
    _stopping = false;
    _release = null;
    // Consume errors immediately, including while native start is pending.
    final outcome = done.future.then<Object>(
      (value) => value,
      onError: (Object error) => error,
    );
    final watchdog = Timer(speechTimeout, () {
      _unavailable = true;
      _finish(
        false,
        error: const SpeechFailure('음성 엔진의 응답 시간이 초과되었습니다. 앱을 다시 열어 주세요.'),
      );
    });
    try {
      _activation = _audio.activate(() {
        unawaited(
          stop().catchError((Object error) => _finish(false, error: error)),
        );
      });
      await _activation;
      if (_stopping) {
        _finish(false);
      } else {
        // AudioSession handles focus/interruption, avoiding two focus owners.
        unawaited(
          client
              .speak(text)
              .then(
                (started) {
                  if (identical(_utterance, done) && started != 1) {
                    _finish(
                      false,
                      error: const SpeechFailure(
                        '읽어주기를 시작하지 못했습니다. 다시 시도해 주세요.',
                      ),
                    );
                  }
                },
                onError: (Object error) {
                  if (identical(_utterance, done)) {
                    _unavailable = true;
                    _finish(false, error: error);
                  }
                },
              ),
        );
      }
      final result = await outcome;
      if (result is! bool) throw result;
      return result;
    } finally {
      watchdog.cancel();
      _finish(false);
      if (_unavailable) await _tts!.stop();
      await _releaseAudio();
      _activation = null;
      if (identical(_utterance, done)) {
        _utterance = null;
        _text = null;
        _onRange = null;
      }
    }
  }

  @override
  Future<void> stop() async {
    if (_tts == null) return;
    _stopping = true;
    // Complete activation before releasing it or handing audio to recording.
    await _activation;
    final pending = _utterance;
    await _tts!.stop();
    if (pending != null) {
      try {
        await pending.future.timeout(const Duration(seconds: 3));
      } on TimeoutException {
        _unavailable = true;
        _finish(false);
        throw const SpeechFailure('음성 중지를 확인하지 못했습니다. 앱을 다시 열어 주세요.');
      }
    }
    await _releaseAudio();
  }
}
