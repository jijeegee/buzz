import 'dart:async';
import 'package:audio_session/audio_session.dart';

/// Audio focus owned by speech, independent of any particular TTS backend.
abstract interface class SpeechAudioSession {
  Future<void> activate(void Function() interrupted);
  Future<void> release();
}

/// Releases focus on pause/stop as well as completion; never auto-resumes after
/// phone calls, headphone removal or another app taking audio focus.
class DeviceSpeechAudioSession implements SpeechAudioSession {
  AudioSession? _session;
  StreamSubscription<AudioInterruptionEvent>? _interruptions;
  StreamSubscription<void>? _noisy;
  bool _owned = false;

  @override
  Future<void> activate(void Function() interrupted) async {
    final session = _session ??= await AudioSession.instance;
    await session.configure(const AudioSessionConfiguration.speech());
    _interruptions ??= session.interruptionEventStream.listen((event) {
      if (event.begin && _owned) interrupted();
    });
    _noisy ??= session.becomingNoisyEventStream.listen((_) {
      if (_owned) interrupted();
    });
    _owned = await session.setActive(true);
    if (!_owned) throw StateError('Audio focus unavailable');
  }

  @override
  Future<void> release() async {
    if (_owned) {
      _owned = false;
      await _session?.setActive(false);
    }
    await _interruptions?.cancel();
    await _noisy?.cancel();
    _interruptions = null;
    _noisy = null;
  }
}
