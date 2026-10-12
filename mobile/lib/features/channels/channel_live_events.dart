import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';

/// Fans the channel list's live events (one subscription across all joined
/// channels) out to other readers, so they can patch their own state without
/// opening a second subscription per channel.
class ChannelLiveEvents {
  final List<void Function(NostrEvent)> _listeners = [];

  /// Registers [listener] and returns a function that removes it.
  void Function() listen(void Function(NostrEvent) listener) {
    _listeners.add(listener);
    return () => _listeners.remove(listener);
  }

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}

final channelLiveEventsProvider = Provider<ChannelLiveEvents>(
  (ref) => ChannelLiveEvents(),
);
