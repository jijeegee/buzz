import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community_provider.dart';
import 'speech_audio_gate.dart';
import 'read_aloud_preferences.dart';
import 'speech_engine.dart';
import 'system_speech_engine.dart';

/// Override this provider to replace system TTS without changing message UI.
final speechEngineProvider = Provider<SpeechEngine>(
  (_) => SystemSpeechEngine(),
);

/// The single speech session shared by all mobile conversations.
final readAloudControllerProvider =
    NotifierProvider<ReadAloudController, ReadAloudState>(
      ReadAloudController.new,
    );

enum ReadAloudPhase { idle, preparing, speaking, paused, failed }

/// A snapshot of the active message and the actual native speech range.
class ReadAloudState {
  const ReadAloudState({
    this.owner,
    this.text = '',
    this.language = 'ko',
    this.phase = ReadAloudPhase.idle,
    this.start = 0,
    this.end = 0,
    this.hasProgress = false,
    this.error,
  });
  final Object? owner;
  final String text;
  final String language;
  final ReadAloudPhase phase;
  final int start;
  final int end;
  final bool hasProgress;
  final String? error;
  bool get active =>
      phase == ReadAloudPhase.preparing ||
      phase == ReadAloudPhase.speaking ||
      phase == ReadAloudPhase.paused;

  ReadAloudState withPhase(ReadAloudPhase phase) => ReadAloudState(
    owner: owner,
    text: text,
    language: language,
    phase: phase,
    start: start,
    end: end,
    hasProgress: hasProgress,
  );
}

/// A conservative chunk boundary; never cuts a UTF-16 surrogate pair.
int speechChunkEnd(String text, int start, {int limit = 600}) {
  var end = (start + limit).clamp(0, text.length);
  if (end == text.length) return end;
  for (var i = end; i > start + limit ~/ 2; i--) {
    if (RegExp(r'\s').hasMatch(text[i - 1])) return i;
  }
  if (end > start &&
      text.codeUnitAt(end - 1) >= 0xd800 &&
      text.codeUnitAt(end - 1) <= 0xdbff) {
    end--;
  }
  return end;
}

/// Serializes native mutations and fences asynchronous speech callbacks.
class ReadAloudController extends Notifier<ReadAloudState> {
  late SpeechEngine _engine;
  Future<void> _tail = Future.value();
  int _generation = 0;
  bool _wordRangesAvailable = false;
  late SpeechAudioGate _audioGate;

  @override
  ReadAloudState build() {
    _engine = ref.read(speechEngineProvider);
    _audioGate = ref.read(speechAudioGateProvider);
    final releaseGate = _audioGate.register(() async {
      await stop();
      if (state.phase == ReadAloudPhase.failed) {
        throw SpeechFailure(state.error!);
      }
    });
    ref.listen(readAloudEnabledProvider, (_, enabled) {
      if (!enabled) unawaited(stop());
    });
    final unregister = ref.read(communityTransitionProvider).register(stop);
    final lifecycle = AppLifecycleListener(
      onStateChange: (phase) {
        if (phase != AppLifecycleState.resumed && state.active) {
          unawaited(pause());
        }
      },
    );
    ref.onDispose(() {
      _generation++;
      unregister();
      releaseGate();
      lifecycle.dispose();
      unawaited(_engine.stop().catchError((Object _) {}));
    });
    return const ReadAloudState();
  }

  bool _current(int generation) => ref.mounted && generation == _generation;

  Future<void> _queue(Future<void> Function() action) {
    final result = _tail.then((_) => action());
    _tail = result.catchError((Object _) {});
    return result;
  }

  /// Claims microphone/media exclusivity until the caller releases it.
  Future<void> acquireAudio(Object owner) async {
    await _audioGate.acquire(owner);
  }

  /// Releases only this caller's audio ownership.
  void releaseAudio(Object owner) => _audioGate.release(owner);

  /// Starts a message, replacing any earlier one. Empty text is never sent.
  Future<void> play(Object owner, String text, String language) async {
    if (!ref.read(readAloudEnabledProvider) || text.trim().isEmpty) return;
    if (_audioGate.busy) {
      state = ReadAloudState(
        owner: owner,
        phase: ReadAloudPhase.failed,
        error: '통화나 녹음을 마친 뒤 읽어주기를 사용해 주세요.',
      );
      return;
    }
    final generation = ++_generation;
    _wordRangesAvailable = false;
    state = ReadAloudState(
      owner: owner,
      text: text,
      language: language,
      phase: ReadAloudPhase.preparing,
    );
    await _begin(generation, 0);
  }

  Future<void> _begin(int generation, int offset) async {
    try {
      await _queue(() async {
        await _engine.stop();
        if (!_current(generation)) return;
        await _engine
            .prepare(state.language)
            .timeout(const Duration(seconds: 10));
      });
      if (!_current(generation)) return;
      state = state.withPhase(ReadAloudPhase.speaking);
      unawaited(_speak(generation, offset));
    } catch (error) {
      _fail(generation, error);
    }
  }

  Future<void> _speak(int generation, int offset) async {
    final text = state.text;
    try {
      while (_current(generation) && offset < text.length) {
        final base = offset;
        final end = speechChunkEnd(
          text,
          base,
          limit: _wordRangesAvailable ? 600 : 80,
        );
        final completed = await _engine.speak(text.substring(base, end), (
          start,
          finish,
        ) {
          if (!_current(generation) ||
              state.phase != ReadAloudPhase.speaking ||
              start < 0 ||
              finish <= start ||
              base + finish > end) {
            return;
          }
          _wordRangesAvailable = true;
          state = ReadAloudState(
            owner: state.owner,
            text: text,
            language: state.language,
            phase: ReadAloudPhase.speaking,
            start: base + start,
            end: base + finish,
            hasProgress: true,
          );
        });
        if (!_current(generation)) return;
        if (!completed) {
          state = state.withPhase(ReadAloudPhase.paused);
          return;
        }
        offset = end;
        if (offset < text.length) {
          state = ReadAloudState(
            owner: state.owner,
            text: text,
            language: state.language,
            phase: ReadAloudPhase.speaking,
            start: offset,
            end: offset,
          );
        }
      }
      if (_current(generation)) state = const ReadAloudState();
    } catch (error) {
      if (_current(generation)) {
        try {
          await _queue(_engine.stop);
        } catch (_) {
          /* reported below */
        }
        _fail(generation, error);
      }
    }
  }

  /// Pauses immediately, retaining the native word boundary for resume.
  Future<void> pause() async {
    if (!state.active || state.phase == ReadAloudPhase.paused) return;
    final generation = ++_generation;
    state = state.withPhase(ReadAloudPhase.paused);
    try {
      await _queue(_engine.stop);
    } catch (error) {
      _fail(generation, error);
    }
  }

  /// Resumes at the last reported word (never skips a partly spoken word).
  Future<void> resume() async {
    if (state.phase != ReadAloudPhase.paused ||
        !ref.read(readAloudEnabledProvider)) {
      return;
    }
    if (_audioGate.busy) {
      return;
    }
    final offset = state.start;
    final generation = ++_generation;
    state = state.withPhase(ReadAloudPhase.preparing);
    await _begin(generation, offset);
  }

  /// Stops playback and clears its position. Replaying starts from the start.
  Future<void> stop() async {
    final generation = ++_generation;
    final previous = state;
    state = const ReadAloudState();
    try {
      await _queue(_engine.stop);
    } catch (error) {
      if (_current(generation)) {
        state = ReadAloudState(
          owner: previous.owner,
          phase: ReadAloudPhase.failed,
          error: _errorText(error),
        );
      }
    }
  }

  /// Cleanup from a row must not stop a newer row's playback.
  Future<void> stopOwner(Object owner) async {
    if (!ref.mounted) return;
    if (identical(state.owner, owner)) await stop();
  }

  void _fail(int generation, Object error) {
    if (!_current(generation)) return;
    state = ReadAloudState(
      owner: state.owner,
      phase: ReadAloudPhase.failed,
      error: _errorText(error),
    );
  }

  String _errorText(Object error) => error is SpeechFailure
      ? error.message
      : '읽어주기를 사용할 수 없습니다. 휴대폰 음성 설정을 확인하고 다시 시도해 주세요.';
}
