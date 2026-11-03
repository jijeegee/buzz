/// Removes push notifications of messages already read, like KakaoTalk:
/// reading a room (on this phone, or on another device once its read
/// markers reach the app) clears that room's notifications from the tray.
library;

import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community_provider.dart';
import '../read_state/message_read_state.dart';
import '../read_state/read_state_format.dart';
import '../read_state/read_state_provider.dart';
import '../relay/app_lifecycle_provider.dart';
import 'push_bridge.dart';
import 'push_thread_target.dart';

/// A message notification in the tray, as the native side reports it.
class DeliveredPushNotification {
  final String key;
  final String eventId;
  final String communityId;
  final String channelId;
  final String? threadRootId;
  final int createdAt;

  const DeliveredPushNotification({
    required this.key,
    required this.eventId,
    required this.communityId,
    required this.channelId,
    required this.createdAt,
    this.threadRootId,
  });

  /// Null for anything malformed: such a notification is left alone.
  static DeliveredPushNotification? fromNative(Map<Object?, Object?> raw) {
    final key = raw['key'];
    final eventId = raw['eventId'];
    final communityId = raw['communityId'];
    final channelId = raw['channelId'];
    final createdAt = raw['createdAt'];
    final threadRootId = raw['threadRootId'];
    if (key is! String ||
        key.isEmpty ||
        eventId is! String ||
        eventId.isEmpty ||
        communityId is! String ||
        communityId.isEmpty ||
        channelId is! String ||
        channelId.isEmpty ||
        createdAt is! int ||
        createdAt <= 0) {
      return null;
    }
    return DeliveredPushNotification(
      key: key,
      eventId: eventId,
      communityId: communityId,
      channelId: channelId,
      createdAt: createdAt,
      threadRootId: isPushEventId(threadRootId) ? threadRootId as String : null,
    );
  }
}

/// Keys of the [delivered] notifications of [communityId] whose message
/// [readState] shows as read — the same rule as the app's unread marks,
/// and anything marked unread on purpose stays.
List<String> readPushNotificationKeys(
  Iterable<DeliveredPushNotification> delivered, {
  required ReadStateState readState,
  required String communityId,
}) {
  if (!readState.isReady) return const [];
  return [
    for (final item in delivered)
      if (item.communityId == communityId && _isRead(item, readState)) item.key,
  ];
}

bool _isRead(DeliveredPushNotification item, ReadStateState readState) {
  final thread = item.threadRootId;
  if (readState.isForcedUnread(item.channelId) ||
      (thread != null && readState.isForcedUnread(threadContextKey(thread)))) {
    return false;
  }
  return !isMessageUnread(
    readState,
    channelId: item.channelId,
    messageId: item.eventId,
    createdAt: item.createdAt,
    threadRootId: thread,
  );
}

/// Sweeps the tray whenever what is read may have changed: the read state
/// (local reads, markers from other devices, account switch), the app
/// coming back, or a notification posted while the app runs (Android).
class PushTraySweeper extends ConsumerStatefulWidget {
  final Widget child;

  const PushTraySweeper({super.key, required this.child});

  @override
  ConsumerState<PushTraySweeper> createState() => _PushTraySweeperState();
}

class _PushTraySweeperState extends ConsumerState<PushTraySweeper> {
  static const _debounce = Duration(milliseconds: 500);
  Timer? _timer;
  bool _running = false;
  bool _again = false;

  @override
  void initState() {
    super.initState();
    pushNotificationsPosted.addListener(_schedule);
    WidgetsBinding.instance.addPostFrameCallback((_) => _schedule());
  }

  @override
  void dispose() {
    _timer?.cancel();
    pushNotificationsPosted.removeListener(_schedule);
    super.dispose();
  }

  void _schedule() {
    if (!mounted) return;
    _timer?.cancel();
    _timer = Timer(_debounce, () => unawaited(_sweep()));
  }

  Future<void> _sweep() async {
    if (_running) {
      _again = true;
      return;
    }
    _running = true;
    try {
      do {
        _again = false;
        await _sweepOnce();
      } while (_again && mounted);
    } finally {
      _running = false;
    }
  }

  Future<void> _sweepOnce() async {
    if (!mounted) return;
    final readState = ref.read(readStateProvider);
    final community = ref.read(activeCommunityProvider).value;
    if (!readState.isReady || community == null) return;
    // Two accounts on one server share room ids: never judge one account's
    // notifications with the other's read state.
    // (Token accounts key their read state by principal, not pubkey.)
    final owner = community.pubkey;
    if (!community.tokenAuth &&
        owner != null &&
        readState.pubkey != null &&
        owner.toLowerCase() != readState.pubkey!.toLowerCase()) {
      return;
    }
    final delivered = [
      for (final raw in await readDeliveredPushNotifications())
        ?DeliveredPushNotification.fromNative(raw),
    ];
    if (!mounted || delivered.isEmpty) return;
    final keys = readPushNotificationKeys(
      delivered,
      readState: ref.read(readStateProvider),
      communityId: community.id,
    );
    await removeDeliveredPushNotifications(keys);
  }

  @override
  Widget build(BuildContext context) {
    ref.listen<ReadStateState>(readStateProvider, (_, _) => _schedule());
    ref.listen(activeCommunityProvider, (_, _) => _schedule());
    ref.listen<AppLifecycleState>(appLifecycleProvider, (_, next) {
      if (next == AppLifecycleState.resumed) _schedule();
    });
    return widget.child;
  }
}
