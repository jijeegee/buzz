import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../../shared/relay/relay.dart';
import '../../../shared/theme/theme_provider.dart';
import '../../channels/channel_management_provider.dart';
import '../../chats/chat_list.dart';
import 'owner_view.dart';

/// What the owner has seen of an agent's conversations, on this device: per
/// chat or thread (read-state context id) the newest message time looked at.
/// The first look at an agent's list sets [baseline], so older chats start
/// without dots. Mirrors desktop's `ownerViewSeen`.
@immutable
class OwnerViewSeen {
  const OwnerViewSeen({required this.baseline, this.seen = const {}});

  /// Unix seconds of the first look at this agent's list.
  final int baseline;

  /// Context id → newest `created_at` looked at.
  final Map<String, int> seen;

  /// Activity at or before this has been seen in [contextId].
  int seenAt(String contextId) => math.max(baseline, seen[contextId] ?? 0);

  /// [contextId] looked at up to [createdAt]; the same record when nothing
  /// is newer.
  OwnerViewSeen markSeen(String contextId, int createdAt) {
    if ((seen[contextId] ?? 0) >= createdAt) return this;
    return OwnerViewSeen(
      baseline: baseline,
      seen: {...seen, contextId: createdAt},
    );
  }

  static OwnerViewSeen? decode(String? raw) {
    if (raw == null || raw.isEmpty) return null;
    try {
      final json = jsonDecode(raw);
      if (json is! Map<String, dynamic>) return null;
      final baseline = json['baseline'];
      if (baseline is! int) return null;
      final seen = json['seen'];
      return OwnerViewSeen(
        baseline: baseline,
        seen: {
          if (seen is Map<String, dynamic>)
            for (final entry in seen.entries)
              if (entry.value is int) entry.key: entry.value as int,
        },
      );
    } on FormatException {
      return null;
    }
  }

  /// JSON with at most [maxContexts] contexts, the oldest dropped.
  String encode({int maxContexts = ownerViewMaxSeenContexts}) {
    var kept = seen;
    if (seen.length > maxContexts) {
      final entries = seen.entries.toList()
        ..sort((left, right) => right.value.compareTo(left.value));
      kept = Map.fromEntries(entries.take(maxContexts));
    }
    return jsonEncode({'baseline': baseline, 'seen': kept});
  }
}

/// Contexts kept per agent; the oldest drop first.
const ownerViewMaxSeenContexts = 500;

/// [rows] with a dot (`unreadMark`, no counts) where someone other than the
/// agent wrote after what the owner has seen. Before the first look at the
/// list ([seen] null) nothing has a dot.
List<ChatListRow> withSeenDots(
  Iterable<ChatListRow> rows,
  String agentPubkey,
  OwnerViewSeen? seen,
) {
  final agent = agentPubkey.toLowerCase();
  return [
    for (final row in rows)
      row.copyWith(
        unread: 0,
        unreadOverflow: false,
        unreadMentions: 0,
        unreadMentionsOverflow: false,
        unreadMark:
            seen != null &&
            row.latest != null &&
            row.latest!.pubkey.toLowerCase() != agent &&
            row.latest!.createdAt > seen.seenAt(chatRowContextId(row)),
      ),
  ];
}

/// The record for the signed-in account and [agentPubkey], saved on this
/// device. Null until the agent's list first loads.
class OwnerViewSeenNotifier extends Notifier<OwnerViewSeen?> {
  OwnerViewSeenNotifier(this.agentPubkey);

  final String agentPubkey;

  String? _key;

  @override
  OwnerViewSeen? build() {
    final config = ref.watch(relayConfigProvider);
    final me = ref.watch(currentPubkeyProvider);
    if (me == null || me.isEmpty) {
      _key = null;
      return null;
    }
    _key =
        'kuzz.ownerView.seen.v1:${storageScopeFor(config.baseUrl)}:'
        '${me.toLowerCase()}:${agentPubkey.toLowerCase()}';
    return OwnerViewSeen.decode(ref.read(savedPrefsProvider).getString(_key!));
  }

  /// Start the baseline on the first look at this agent's list.
  void ensureBaseline({DateTime? now}) {
    if (state != null) return;
    final at = (now ?? DateTime.now()).millisecondsSinceEpoch ~/ 1000;
    _save(OwnerViewSeen(baseline: at));
  }

  /// The owner looked at [contextId] up to [createdAt].
  void markSeen(String contextId, int createdAt) {
    final current = state ?? const OwnerViewSeen(baseline: 0);
    final next = current.markSeen(contextId, createdAt);
    if (identical(next, current) && state != null) return;
    _save(next);
  }

  void _save(OwnerViewSeen record) {
    state = record;
    final key = _key;
    if (key == null) return;
    // A failed write only brings the dots back next time.
    unawaited(ref.read(savedPrefsProvider).setString(key, record.encode()));
  }
}

/// What the signed-in owner has seen of [agentPubkey]'s conversations.
final ownerViewSeenProvider =
    NotifierProvider.family<OwnerViewSeenNotifier, OwnerViewSeen?, String>(
      OwnerViewSeenNotifier.new,
    );

/// [agentPubkey]'s conversations with their dots. Loading the list starts
/// the baseline.
final agentChatRowsProvider = Provider.autoDispose
    .family<AsyncValue<List<ChatListRow>>, String>((ref, agentPubkey) {
      final agent = agentPubkey.toLowerCase();
      final list = ref.watch(agentChatListProvider(agent));
      final seen = ref.watch(ownerViewSeenProvider(agent));
      final rows = list.value;
      if (rows == null) return list;
      if (seen == null) {
        // Out of the build: the first look at the list starts the baseline.
        final notifier = ref.read(ownerViewSeenProvider(agent).notifier);
        Future.microtask(notifier.ensureBaseline);
      }
      return AsyncData(withSeenDots(rows, agent, seen));
    });

/// Rows of [agentPubkey]'s conversations with a dot: the count on the tab.
int ownerViewDotCount(Iterable<ChatListRow> rows) =>
    rows.where((row) => row.unreadMark).length;

/// Marks the open chat's rows seen at their latest message, now and as the
/// list updates: [isOpen] picks them (the room row for a chat page, the
/// thread's row for a thread page).
void useMarkOwnerViewSeen(
  WidgetRef ref,
  OwnerView? view,
  bool Function(ChatListRow row) isOpen,
) {
  final agent = view?.agentPubkey.toLowerCase();
  final rows = agent == null
      ? null
      : ref.watch(agentChatListProvider(agent)).value;
  useEffect(() {
    if (agent == null || rows == null) return null;
    final open = [
      for (final row in rows)
        if (row.latest != null && isOpen(row)) row,
    ];
    if (open.isEmpty) return null;
    // Out of the build: the seen record is provider state.
    final seen = ref.read(ownerViewSeenProvider(agent).notifier);
    Future.microtask(() {
      for (final row in open) {
        seen.markSeen(chatRowContextId(row), row.latest!.createdAt);
      }
    });
    return null;
  }, [agent, rows]);
}
