import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'pending_local_messages_provider.dart';

/// Where an own message stands before the relay has it.
enum LocalSendStatus {
  /// Accepted by the relay, or not a local send at all.
  sent,

  /// Signed and shown, waiting for the relay to accept it.
  pending,

  /// The relay rejected it or the publish failed; kept on screen for retry.
  failed,
}

/// Ids of local messages whose publish failed after they were shown.
///
/// A failed message stays in [pendingLocalMessagesProvider] so the timeline,
/// thread overlays and dedup logic keep treating it as an unconfirmed local
/// row. This set only marks which of those rows failed, and it never outlives
/// the pending entry: once a message is confirmed, accepted or deleted, its
/// failed mark drops with it.
class FailedLocalMessagesNotifier extends Notifier<Set<String>> {
  final String channelId;

  FailedLocalMessagesNotifier(this.channelId);

  @override
  Set<String> build() {
    ref.listen(pendingLocalMessagesProvider(channelId), (_, pending) {
      if (state.every(pending.containsKey)) return;
      state = {
        for (final id in state)
          if (pending.containsKey(id)) id,
      };
    });
    return const {};
  }

  /// Marks a still-pending local message as failed.
  void markFailed(String eventId) {
    if (state.contains(eventId)) return;
    if (!ref
        .read(pendingLocalMessagesProvider(channelId))
        .containsKey(eventId)) {
      return;
    }
    state = {...state, eventId};
  }

  /// Returns a failed message to pending, e.g. when it is retried.
  void clear(String eventId) {
    if (!state.contains(eventId)) return;
    state = {...state}..remove(eventId);
  }
}

final failedLocalMessagesProvider =
    NotifierProvider.family<FailedLocalMessagesNotifier, Set<String>, String>(
      FailedLocalMessagesNotifier.new,
    );

/// Watches the send status of [eventId] in [channelId], rebuilding only when
/// that message's status changes.
LocalSendStatus watchLocalSendStatus(
  WidgetRef ref,
  String channelId,
  String eventId,
) {
  final pending = ref.watch(
    pendingLocalMessagesProvider(
      channelId,
    ).select((messages) => messages.containsKey(eventId)),
  );
  final failed = ref.watch(
    failedLocalMessagesProvider(
      channelId,
    ).select((ids) => ids.contains(eventId)),
  );
  if (!pending) return LocalSendStatus.sent;
  return failed ? LocalSendStatus.failed : LocalSendStatus.pending;
}
