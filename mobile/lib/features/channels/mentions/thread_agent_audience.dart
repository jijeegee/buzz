import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../../shared/theme/theme_provider.dart';

/// Device-local "Automatically mention agents" preference. Mirrors desktop's
/// `autoPinMentionedAgentsPreference.ts`: off by default, and turning it off
/// forgets every thread's automatically mentioned agents.
final keepMentionedAgentsPinnedProvider =
    NotifierProvider<KeepMentionedAgentsPinnedPreference, bool>(
      KeepMentionedAgentsPinnedPreference.new,
    );

class KeepMentionedAgentsPinnedPreference extends Notifier<bool> {
  static const key = 'buzz_mobile_keep_mentioned_agents_pinned';
  static const defaultValue = false;

  @override
  bool build() => ref.watch(savedPrefsProvider).getBool(key) ?? defaultValue;

  Future<void> setEnabled(bool enabled) async {
    if (!enabled) ref.read(threadAgentAudienceProvider.notifier).reset();
    if (enabled == state) return;
    state = enabled;
    // Persistence is best-effort; the live preference still applies.
    await ref.read(savedPrefsProvider).setBool(key, enabled);
  }
}

/// Audience scope for one thread composer, matching desktop's
/// `owner:channel:thread:<headId>` key. Null outside threads.
String? threadAgentAudienceScope({
  required String? ownerPubkey,
  required String channelId,
  required String? threadHeadId,
}) {
  final owner = ownerPubkey?.trim().toLowerCase();
  if (owner == null ||
      !_hexPubkey.hasMatch(owner) ||
      channelId.isEmpty ||
      threadHeadId == null ||
      threadHeadId.isEmpty) {
    return null;
  }
  return '$owner:$channelId:thread:$threadHeadId';
}

final _hexPubkey = RegExp(r'^[0-9a-f]{64}$');

@immutable
class ThreadAgentAudienceState {
  final Map<String, List<String>> audiences;
  final Map<String, Set<String>> excluded;

  const ThreadAgentAudienceState({
    this.audiences = const {},
    this.excluded = const {},
  });

  List<String> pubkeysFor(String? scope) =>
      scope == null ? const [] : audiences[scope] ?? const [];
}

/// In-memory automatically mentioned agents per thread, like desktop's
/// `persistentAgentAudience.ts`. Agents the user removes in a thread are
/// remembered as excluded so the thread root does not add them back.
final threadAgentAudienceProvider =
    NotifierProvider<ThreadAgentAudience, ThreadAgentAudienceState>(
      ThreadAgentAudience.new,
    );

class ThreadAgentAudience extends Notifier<ThreadAgentAudienceState> {
  static const maxScopes = 200;

  @override
  ThreadAgentAudienceState build() => const ThreadAgentAudienceState();

  void reset() => state = const ThreadAgentAudienceState();

  /// Adds the thread root's agents unless already present or excluded.
  void initialize(String scope, Iterable<String> pubkeys) {
    promote(scope, pubkeys, reinstateExcluded: false);
  }

  /// Adds [pubkeys] and returns the ones that were newly added.
  List<String> promote(
    String scope,
    Iterable<String> pubkeys, {
    required bool reinstateExcluded,
  }) {
    final current = state.pubkeysFor(scope);
    final excluded = state.excluded[scope] ?? const <String>{};
    final promoted = [
      for (final pubkey in _normalize(pubkeys))
        if (!current.contains(pubkey) &&
            (reinstateExcluded || !excluded.contains(pubkey)))
          pubkey,
    ];
    if (promoted.isEmpty) return const [];
    _write(
      scope,
      [...current, ...promoted],
      excluded: reinstateExcluded
          ? excluded.difference(promoted.toSet())
          : null,
    );
    return promoted;
  }

  /// Stops automatically mentioning [pubkey] in this thread.
  void exclude(String scope, String pubkey) {
    final normalized = _normalize([pubkey]).firstOrNull;
    if (normalized == null) return;
    _write(
      scope,
      state.pubkeysFor(scope).where((p) => p != normalized).toList(),
      excluded: {...?state.excluded[scope], normalized},
    );
  }

  void _write(String scope, List<String> pubkeys, {Set<String>? excluded}) {
    final audiences = Map.of(state.audiences)
      ..remove(scope)
      ..[scope] = List.unmodifiable(pubkeys);
    while (audiences.length > maxScopes) {
      audiences.remove(audiences.keys.first);
    }
    final nextExcluded = Map.of(state.excluded);
    if (excluded != null) {
      if (excluded.isEmpty) {
        nextExcluded.remove(scope);
      } else {
        nextExcluded[scope] = Set.unmodifiable(excluded);
      }
    }
    nextExcluded.removeWhere((key, _) => !audiences.containsKey(key));
    state = ThreadAgentAudienceState(
      audiences: Map.unmodifiable(audiences),
      excluded: Map.unmodifiable(nextExcluded),
    );
  }
}

List<String> _normalize(Iterable<String> pubkeys) => [
  ...{for (final pubkey in pubkeys) pubkey.trim().toLowerCase()},
].where(_hexPubkey.hasMatch).toList();
