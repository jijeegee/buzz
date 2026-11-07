import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';

/// Held by one account for all of them before this was per account; what is
/// left of it cannot be told apart, so it is deleted.
const _sharedStateKey = 'activity_inbox_local_state_v1';
const _maxTrackedIds = 500;

/// Local, per-account inbox row overrides — mirrors desktop's
/// `useFeedItemState`:
/// - [doneIds] marks channel-less rows (e.g. approvals without an `h` tag)
///   as read, since they have no NIP-RS context to advance.
/// - [unreadIds] is the per-row "mark unread" override that reopens a row
///   without rewinding the shared channel/thread read marker.
@immutable
class InboxLocalState {
  final Set<String> doneIds;
  final Set<String> unreadIds;

  const InboxLocalState({required this.doneIds, required this.unreadIds});

  const InboxLocalState.empty() : doneIds = const {}, unreadIds = const {};
}

class InboxLocalStateNotifier extends Notifier<InboxLocalState> {
  String? _key;

  @override
  InboxLocalState build() {
    final prefs = ref.read(savedPrefsProvider);
    if (prefs.containsKey(_sharedStateKey)) {
      unawaited(prefs.remove(_sharedStateKey));
    }
    final baseUrl = ref.watch(relayConfigProvider.select((c) => c.baseUrl));
    final pubkey = ref.watch(myPubkeyProvider)?.toLowerCase();
    final key = _key = pubkey == null
        ? null
        : '$_sharedStateKey:${storageScopeFor(baseUrl)}:$pubkey';
    final raw = key == null ? null : prefs.getString(key);
    if (raw == null) return const InboxLocalState.empty();
    try {
      final decoded = jsonDecode(raw);
      if (decoded is! Map<String, dynamic>) {
        return const InboxLocalState.empty();
      }
      Set<String> readIds(Object? value) =>
          value is List ? value.whereType<String>().toSet() : const <String>{};
      return InboxLocalState(
        doneIds: readIds(decoded['done']),
        unreadIds: readIds(decoded['unread']),
      );
    } catch (_) {
      return const InboxLocalState.empty();
    }
  }

  void markDone(String id) {
    _update(
      doneIds: {...state.doneIds, id},
      unreadIds: {...state.unreadIds}..remove(id),
    );
  }

  void markUnread(Iterable<String> ids) {
    _update(
      doneIds: {...state.doneIds}..removeAll(ids),
      unreadIds: {...state.unreadIds, ...ids},
    );
  }

  void clearUnread(Iterable<String> ids) {
    if (!ids.any(state.unreadIds.contains)) return;
    _update(
      doneIds: state.doneIds,
      unreadIds: {...state.unreadIds}..removeAll(ids),
    );
  }

  void _update({required Set<String> doneIds, required Set<String> unreadIds}) {
    Set<String> capped(Set<String> ids) => ids.length <= _maxTrackedIds
        ? ids
        : ids.skip(ids.length - _maxTrackedIds).toSet();
    state = InboxLocalState(
      doneIds: capped(doneIds),
      unreadIds: capped(unreadIds),
    );
    final key = _key;
    if (key == null) return;
    final prefs = ref.read(savedPrefsProvider);
    prefs.setString(
      key,
      jsonEncode({
        'done': state.doneIds.toList(),
        'unread': state.unreadIds.toList(),
      }),
    );
  }
}

final inboxLocalStateProvider =
    NotifierProvider<InboxLocalStateNotifier, InboxLocalState>(
      InboxLocalStateNotifier.new,
    );
