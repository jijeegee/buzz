import 'dart:async';

import 'package:flutter/scheduler.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'channel_preload.dart';

/// Startup store upkeep waits at most this long for the room on screen.
const deferredStoreWorkRoomCap = Duration(seconds: 5);

const _roomPoll = Duration(milliseconds: 100);

/// Device-store upkeep that draws nothing (forgetting the rooms the account
/// left) runs off the startup path: after the first frame, after the first
/// window of the room on screen (at most [deferredStoreWorkRoomCap]), and
/// then each piece as its own scheduler task between frames, in the order
/// queued. Not [Priority.idle]: looping animations (a spinner, the agent
/// working indicator) keep the scheduler from ever idling, and an idle task
/// then re-arms the event loop without end. Without a Flutter
/// binding (plain unit tests) work runs at once.
class DeferredStoreWork {
  DeferredStoreWork(this._ref);

  final Ref _ref;
  final List<void Function(Ref ref)> _queue = [];
  bool _startupDone = false;
  bool _draining = false;

  /// Queues [work]; it gets this provider's [Ref], which outlives the caller.
  void run(void Function(Ref ref) work) {
    final binding = _schedulerBinding();
    if (binding == null) {
      work(_ref);
      return;
    }
    _queue.add(work);
    if (!_draining) unawaited(_drain(binding));
  }

  Future<void> _drain(SchedulerBinding binding) async {
    _draining = true;
    try {
      if (!_startupDone) {
        await binding.endOfFrame;
        final capAt = DateTime.now().add(deferredStoreWorkRoomCap);
        while (_ref.mounted &&
            _ref.read(channelPreloaderProvider).visibleRoomLoading &&
            DateTime.now().isBefore(capAt)) {
          await Future<void>.delayed(_roomPoll);
        }
        _startupDone = true;
      }
      while (_queue.isNotEmpty && _ref.mounted) {
        final work = _queue.removeAt(0);
        await binding.scheduleTask(() {
          if (_ref.mounted) work(_ref);
        }, Priority.animation);
      }
    } finally {
      _draining = false;
    }
  }
}

SchedulerBinding? _schedulerBinding() {
  try {
    return SchedulerBinding.instance;
  } catch (_) {
    return null;
  }
}

final deferredStoreWorkProvider = Provider<DeferredStoreWork>(
  DeferredStoreWork.new,
);
