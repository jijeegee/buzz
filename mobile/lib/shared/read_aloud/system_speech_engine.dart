import 'dart:async';
import 'dart:io';

import 'package:flutter_tts/flutter_tts.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../l10n/l10n.dart';

import 'read_aloud_preferences.dart';
import 'speech_audio_session.dart';
import 'speech_engine.dart';

/// One native client for discovery and playback (the plugin shares a channel).
final systemSpeechEngineProvider = Provider<SystemSpeechEngine>(
  (ref) => SystemSpeechEngine(
    selectedEngine: () => ref.read(readAloudEngineProvider),
  ),
);

/// Friendly names for common engines; other installed packages remain usable.
String speechEngineLabel(AppLocalizations l10n, String engine) =>
    switch (engine) {
      '' => l10n.readAloudEnginePhoneDefault,
      'com.google.android.tts' => l10n.readAloudEngineGoogle,
      'com.samsung.SMT' => l10n.readAloudEngineSamsung,
      _ => engine,
    };

/// Free system TTS adapter. Speech text never goes to a Kuzz/cloud endpoint.
class SystemSpeechEngine implements SpeechEngine {
  SystemSpeechEngine({
    SpeechAudioSession? audio,
    bool? android,
    this.selectedEngine,
    this.setupTimeout = const Duration(seconds: 8),
    this.speechTimeout = const Duration(minutes: 2),
  }) : _audio = audio ?? DeviceSpeechAudioSession(),
       _android = android ?? Platform.isAndroid;

  final SpeechAudioSession _audio;
  final bool _android;
  final String Function()? selectedEngine;
  String? _activeEngine;
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
      throw SpeechFailure(appL10n.readAloudEngineNotResponding);
    }
  }

  FlutterTts get _client {
    _checkAvailable();
    return _tts ??= FlutterTts()
      ..setCompletionHandler(() => _finish(true))
      ..setCancelHandler(() => _finish(false))
      ..setErrorHandler(
        (_) => _finish(false, error: SpeechFailure(appL10n.readAloudFailed)),
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

  /// Reads installed Android engines without switching the playback engine.
  Future<List<String>> installedEngines() async {
    if (!_android) return [];
    final result = await _client.getEngines.timeout(setupTimeout);
    _checkAvailable();
    if (result is! List) {
      throw SpeechFailure(appL10n.readAloudEngineListUnavailable);
    }
    return result
        .whereType<String>()
        .where((id) => id.isNotEmpty)
        .toSet()
        .toList()
      ..sort();
  }

  Future<void> _selectEngine(FlutterTts client) async {
    if (!_android || selectedEngine == null) return;
    var requested = selectedEngine!();
    if (requested.isEmpty) {
      final systemDefault = await client.getDefaultEngine;
      _checkAvailable();
      if (systemDefault is! String || systemDefault.isEmpty) {
        throw SpeechFailure(appL10n.readAloudNoDefaultEngine);
      }
      requested = systemDefault;
    }
    // Recheck even when cached: the user may have removed an engine externally.
    if (!(await installedEngines()).contains(requested)) {
      throw SpeechFailure(appL10n.readAloudEngineNotInstalled);
    }
    if (_activeEngine == requested) return;
    _activeEngine = null;
    // flutter_tts discards the native success value; initialization failures
    // propagate as PlatformException, while a successful Future returns null.
    await client.setEngine(requested);
    _checkAvailable();
    _activeEngine = requested;
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
    await _selectEngine(client);
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
            ? (_android
                  ? appL10n.readAloudNoKoreanVoiceAndroid
                  : appL10n.readAloudNoKoreanVoiceIos)
            : appL10n.readAloudNoOfflineVoice,
      );
    }
    if (await client.setVoice(voice) != 1) {
      throw SpeechFailure(appL10n.readAloudVoiceUnavailable);
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
      _finish(false, error: SpeechFailure(appL10n.readAloudEngineTimeout));
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
                      error: SpeechFailure(appL10n.readAloudStartFailed),
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
        throw SpeechFailure(appL10n.readAloudStopFailed);
      }
    }
    await _releaseAudio();
  }
}
