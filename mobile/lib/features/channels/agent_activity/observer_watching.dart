import 'dart:async';
import 'dart:convert';

import 'package:flutter/widgets.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../../../shared/crypto/nip44.dart';
import '../../../shared/relay/relay.dart';

/// Interval between `watching` control frames while the activity view is
/// open. The executor keeps tier-level detail for 60 s after the last one,
/// so one lost frame does not drop the view to summaries.
const observerWatchingInterval = Duration(seconds: 30);

/// Build the owner→agent `watching` control frame: kind 24200 tagged
/// `p`/`agent` = agent and `frame` = `control`, with the NIP-44 encrypted
/// payload `{"type":"watching","channelId":...}`, signed with [nsec].
NostrEvent buildWatchingControlEvent({
  required String nsec,
  required String agentPubkey,
  required String channelId,
  int? createdAt,
}) {
  final privHex = nostr.Nip19.decode(payload: nsec).data;
  if (privHex.isEmpty) throw const FormatException('Invalid nsec');
  final agent = agentPubkey.toLowerCase();
  final content = nip44Encrypt(
    getConversationKey(privHex, agent),
    jsonEncode({'type': 'watching', 'channelId': channelId}),
  );
  return buildOutgoingEvent(
    RelayConfig(baseUrl: '', nsec: nsec),
    kind: EventKind.agentObserverFrame,
    content: content,
    tags: [
      ['p', agent],
      ['agent', agent],
      ['frame', 'control'],
    ],
    createdAt: createdAt,
  );
}

/// Send `watching` for [agentPubkey] while the calling view is mounted, the
/// app is in the foreground and the relay session is connected: once at
/// once, then every [observerWatchingInterval]. Leaving any of those states
/// stops the timer; returning to all of them (e.g. a reconnect) sends again
/// immediately. A failed send is not retried on its own — the next tick is
/// the retry.
void useObserverWatching(
  WidgetRef ref, {
  required String channelId,
  required String agentPubkey,
}) {
  final foreground =
      ref.watch(appLifecycleProvider) == AppLifecycleState.resumed;
  final connected = ref.watch(
    relaySessionProvider.select(
      (session) => session.status == SessionStatus.connected,
    ),
  );
  final config = ref.watch(relayConfigProvider);
  // Encrypting to the agent needs the owner's own key; token communities
  // cannot observe either.
  final nsec = config.tokenAuth ? null : config.nsec;
  final active = foreground && connected && nsec != null && nsec.isNotEmpty;

  useEffect(() {
    final key = nsec;
    if (!active || key == null) return null;
    final session = ref.read(relaySessionProvider.notifier);
    void report(Object error) =>
        debugPrint('Observer watching signal not sent: $error');
    void send() {
      final NostrEvent event;
      try {
        event = buildWatchingControlEvent(
          nsec: key,
          agentPubkey: agentPubkey,
          channelId: channelId,
        );
      } catch (error) {
        report(error);
        return;
      }
      unawaited(session.publish(event).then((_) {}, onError: report));
    }

    send();
    final timer = Timer.periodic(observerWatchingInterval, (_) => send());
    return timer.cancel;
  }, [active, nsec, channelId, agentPubkey]);
}
