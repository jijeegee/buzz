import 'dart:async';
import 'package:buzz/features/channels/voice_note_attachment.dart';
import 'package:buzz/features/channels/voice_note_recording.dart';
import 'package:buzz/shared/read_aloud/speech_audio_gate.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

class LoadingPlayer extends VoiceNotePlayerController {
  @override
  VoiceNotePlaybackState state = const VoiceNotePlaybackState();
  final loaded = Completer<void>();
  bool cancelled = false;
  void Function()? onLoadedIdle;
  int toggles = 0;
  @override
  Future<void> loadRemote(
    String url, {
    required Map<String, String> Function() headers,
    required Duration fallbackDuration,
  }) async {}
  @override
  Future<void> loadLocal(
    String path, {
    required Duration fallbackDuration,
  }) async {}
  @override
  Future<void> toggle() async {
    toggles++;
    if (state.isLoading) {
      cancelled = true;
      state = state.copyWith(isLoading: false, canCancelLoading: false);
      notifyListeners();
      loaded.complete();
      return;
    }
    state = state.copyWith(isLoading: true, canCancelLoading: true);
    notifyListeners();
    await loaded.future;
    if (cancelled) return;
    // The device player reports load completion before starting playback.
    state = state.copyWith(isLoading: false, canCancelLoading: false);
    notifyListeners();
    onLoadedIdle?.call();
    state = state.copyWith(isLoading: false, isPlaying: true);
    notifyListeners();
  }

  @override
  Future<void> pause() async {
    state = state.copyWith(isPlaying: false);
    notifyListeners();
  }

  @override
  Future<void> seek(Duration position) async {}
  @override
  Future<void> setSpeed(double speed) async {}
}

void main() {
  test('same-owner acquisitions share the pending native handoff', () async {
    final gate = SpeechAudioGate();
    final stop = Completer<void>();
    gate.register(() => stop.future);
    final owner = Object();
    var completed = 0;
    final first = gate.acquire(owner).then((_) => completed++);
    final second = gate.acquire(owner).then((_) => completed++);
    await Future<void>.delayed(Duration.zero);
    expect(completed, 0);
    expect(gate.busy, true);
    stop.complete();
    await Future.wait([first, second]);
    expect(completed, 2);
    gate.release(owner);
    expect(gate.busy, false);
  });

  testWidgets(
    'remote note retains ownership through loading and prevents double tap',
    (tester) async {
      final gate = SpeechAudioGate();
      final stopped = Completer<void>();
      gate.register(() => stopped.future);
      final player = LoadingPlayer();
      player.onLoadedIdle = () => expect(gate.busy, true);
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            speechAudioGateProvider.overrideWithValue(gate),
            voiceNotePlayerFactoryProvider.overrideWithValue(() => player),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: const Scaffold(
              body: VoiceNoteAttachment.remote(
                url: 'https://example.test/note.m4a',
                duration: Duration(seconds: 10),
              ),
            ),
          ),
        ),
      );
      final play = find.byKey(const ValueKey('voice-note-play-pause'));
      await tester.tap(play);
      await tester.tap(play);
      expect(gate.busy, true);
      expect(player.toggles, 0);
      stopped.complete();
      await tester.pump();
      expect(player.state.isLoading, true);
      expect(gate.busy, true);
      player.loaded.complete();
      await tester.pump();
      expect(player.toggles, 1);
      expect(player.state.isPlaying, true);
      expect(gate.busy, true);
      await player.pause();
      await tester.pump();
      expect(gate.busy, false);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('remote loading remains cancellable after audio handoff', (
    tester,
  ) async {
    final gate = SpeechAudioGate();
    final player = LoadingPlayer();
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          speechAudioGateProvider.overrideWithValue(gate),
          voiceNotePlayerFactoryProvider.overrideWithValue(() => player),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: const Scaffold(
            body: VoiceNoteAttachment.remote(
              url: 'https://example.test/note.m4a',
              duration: Duration(seconds: 10),
            ),
          ),
        ),
      ),
    );
    final play = find.byKey(const ValueKey('voice-note-play-pause'));
    await tester.tap(play);
    await tester.pump();
    expect(player.state.canCancelLoading, true);
    expect(gate.busy, true);
    await tester.tap(play);
    await tester.pump();
    expect(player.cancelled, true);
    expect(player.toggles, 2);
    expect(player.state.isPlaying, false);
    expect(gate.busy, false);
    expect(tester.takeException(), null);
    await tester.pumpWidget(const SizedBox.shrink());
  });

  testWidgets('handoff error is shown and never starts a voice note', (
    tester,
  ) async {
    final gate = SpeechAudioGate();
    gate.register(() async => throw StateError('native stop failed'));
    final player = LoadingPlayer();
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          speechAudioGateProvider.overrideWithValue(gate),
          voiceNotePlayerFactoryProvider.overrideWithValue(() => player),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: const Scaffold(
            body: VoiceNoteAttachment.remote(
              url: 'https://example.test/note.m4a',
              duration: Duration(seconds: 10),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.byKey(const ValueKey('voice-note-play-pause')));
    await tester.pump();
    expect(player.toggles, 0);
    expect(gate.busy, false);
    expect(find.text('음성 재생을 시작하지 못했습니다. 다시 시도해 주세요.'), findsOneWidget);
    expect(tester.takeException(), null);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
