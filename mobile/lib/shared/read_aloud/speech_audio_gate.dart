import 'dart:async';
import 'package:hooks_riverpod/hooks_riverpod.dart';

/// Coordinates native audio handoff without importing feature modules.
final speechAudioGateProvider = Provider<SpeechAudioGate>(
  (_) => SpeechAudioGate(),
);

/// Recording/huddles claim this before configuring audio, and release afterward.
class SpeechAudioGate {
  final Set<Object> _owners = {};
  final Map<Object, Future<void>> _pending = {};
  final Set<Future<void> Function()> _stoppers = {};
  bool get busy => _owners.isNotEmpty;

  void Function() register(Future<void> Function() stop) {
    _stoppers.add(stop);
    return () => _stoppers.remove(stop);
  }

  Future<void> acquire(Object owner) {
    final pending = _pending[owner];
    if (pending != null) return pending;
    if (!_owners.add(owner)) return Future.value();
    final done = Completer<void>();
    _pending[owner] = done.future;
    unawaited(_acquire(owner, done));
    return done.future;
  }

  Future<void> _acquire(Object owner, Completer<void> done) async {
    try {
      await Future.wait(_stoppers.map((stop) => stop()));
      done.complete();
    } catch (error, stack) {
      _owners.remove(owner);
      done.completeError(error, stack);
    } finally {
      _pending.remove(owner);
    }
  }

  void release(Object owner) => _owners.remove(owner);
}
