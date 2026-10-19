import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'agent_settings.dart';

/// The relay calls agent settings make; a seam for tests.
abstract interface class AgentSettingsRelay {
  Future<List<NostrEvent>> fetch(NostrFilter filter);

  /// Build, sign and publish an event as the signed-in user.
  Future<void> publish({
    required int kind,
    required String content,
    required List<List<String>> tags,
    required int createdAt,
  });
}

class _SessionAgentSettingsRelay implements AgentSettingsRelay {
  const _SessionAgentSettingsRelay(this._ref);
  final Ref _ref;

  @override
  Future<List<NostrEvent>> fetch(NostrFilter filter) =>
      _ref.read(relaySessionProvider.notifier).fetchHistory(filter);

  @override
  Future<void> publish({
    required int kind,
    required String content,
    required List<List<String>> tags,
    required int createdAt,
  }) async {
    final event = buildOutgoingEvent(
      _ref.read(relayConfigProvider),
      kind: kind,
      content: content,
      tags: tags,
      createdAt: createdAt,
    );
    await _ref.read(relaySessionProvider.notifier).publish(event);
  }
}

final agentSettingsRelayProvider = Provider<AgentSettingsRelay>(
  _SessionAgentSettingsRelay.new,
);

/// What the phone can do with one agent's settings.
sealed class AgentSettingsState {
  const AgentSettingsState();
}

/// The phone cannot edit this agent (not on the relay, not the signed-in
/// owner's, or published by an older computer); [message] says why.
class AgentSettingsReadOnly extends AgentSettingsState {
  const AgentSettingsReadOnly({
    required this.name,
    this.message = agentSettingsReadOnlyMessage,
  });
  final String name;
  final String message;
}

/// The agent's own settings (kind:30177), ready to edit.
class AgentSettingsEditable extends AgentSettingsState {
  const AgentSettingsEditable({
    required this.owner,
    required this.agentPubkey,
    required this.event,
    required this.content,
    required this.agentName,
  });

  /// The agent's name as shown (falls back to its profile name).
  final String agentName;

  final String owner;
  final String agentPubkey;

  /// The agent event as loaded; a save refuses if it was replaced.
  final NostrEvent event;
  final Map<String, dynamic> content;

  /// The `d` tag of the preset the agent was made from, if any. A label
  /// only: the agent may keep sharing that preset's name.
  String? get sourcePresetD => switch (sourcePresetIdOf(content)) {
    final id? => personaDTag(id),
    null => null,
  };

  AgentSettingsDraft get draft => AgentSettingsDraft.fromContent(content);
}

/// A save the user has to fix or retry; [message] is shown as is.
class AgentSettingsException implements Exception {
  const AgentSettingsException(this.message);
  final String message;

  @override
  String toString() => message;
}

const agentSettingsReadOnlyMessage =
    'This agent\'s settings can only be changed on the computer it runs on.';

const agentSettingsUpdateComputerMessage =
    'Update Kuzz on your computer to edit this agent here.';

const agentSettingsChangedMessage =
    'Settings changed on another device. Reopen to see the latest.';

/// Loads the editable settings of the agent with this pubkey, or
/// [AgentSettingsReadOnly] named after its kind:30177 event (falls back to
/// [AgentSettingsArgs.fallbackName]).
final agentSettingsProvider = FutureProvider.autoDispose
    .family<AgentSettingsState, AgentSettingsArgs>((ref, args) async {
      final owner = ref.watch(myPubkeyProvider)?.toLowerCase();
      if (owner == null || owner.isEmpty) {
        return AgentSettingsReadOnly(name: args.fallbackName);
      }
      final relay = ref.read(agentSettingsRelayProvider);
      final agentPubkey = args.agentPubkey.toLowerCase();
      final agentEvent = await _fetchAgent(relay, owner, agentPubkey);
      final content = agentEvent == null
          ? null
          : decodeJsonObject(agentEvent.content);
      final agentName = content?['name'];
      final name = agentName is String && agentName.trim().isNotEmpty
          ? agentName.trim()
          : args.fallbackName;
      if (agentEvent == null || content == null) {
        return AgentSettingsReadOnly(name: name);
      }
      if (!agentContentEditable(content)) {
        return AgentSettingsReadOnly(
          name: name,
          message: agentSettingsUpdateComputerMessage,
        );
      }
      return AgentSettingsEditable(
        owner: owner,
        agentPubkey: agentPubkey,
        event: agentEvent,
        content: content,
        agentName: name,
      );
    });

/// Identifies the agent [agentSettingsProvider] loads.
@immutable
class AgentSettingsArgs {
  const AgentSettingsArgs({
    required this.agentPubkey,
    required this.fallbackName,
  });

  final String agentPubkey;
  final String fallbackName;

  @override
  bool operator ==(Object other) =>
      other is AgentSettingsArgs &&
      other.agentPubkey == agentPubkey &&
      other.fallbackName == fallbackName;

  @override
  int get hashCode => Object.hash(agentPubkey, fallbackName);
}

Future<NostrEvent?> _fetchAgent(
  AgentSettingsRelay relay,
  String owner,
  String agentPubkey,
) async {
  final events = await relay.fetch(
    NostrFilter(
      kinds: const [EventKind.managedAgent],
      authors: [owner],
      tags: {
        '#d': [agentPubkey],
      },
      limit: 10,
    ),
  );
  return newestByDTag(
    events,
    kind: EventKind.managedAgent,
    owner: owner,
  )[agentPubkey];
}

/// Publish [draft] over [loaded]'s agent event. Throws
/// [AgentSettingsException] with a user-facing message when the draft is
/// invalid, the name is taken, or the agent changed since it loaded.
Future<void> saveAgentSettings(
  AgentSettingsRelay relay,
  AgentSettingsEditable loaded,
  AgentSettingsDraft draft, {
  DateTime? now,
}) async {
  final invalid = validateAgentSettings(draft);
  if (invalid != null) throw AgentSettingsException(invalid);

  final head = await _fetchAgent(relay, loaded.owner, loaded.agentPubkey);
  if (head == null || head.id != loaded.event.id) {
    throw const AgentSettingsException(agentSettingsChangedMessage);
  }

  if (draft.name.trim() != loaded.draft.name.trim()) {
    List<NostrEvent>? agentEvents;
    List<NostrEvent>? personaEvents;
    try {
      agentEvents = await relay.fetch(
        NostrFilter(
          kinds: const [EventKind.managedAgent],
          authors: [loaded.owner],
          limit: 500,
        ),
      );
      personaEvents = await relay.fetch(
        NostrFilter(
          kinds: const [EventKind.persona],
          authors: [loaded.owner],
          limit: 500,
        ),
      );
    } catch (error) {
      // The computer applying the change is the final word; don't block.
      debugPrint('[AgentSettings] name check skipped: $error');
    }
    if (agentEvents != null &&
        personaEvents != null &&
        agentNameConflicts(
          name: draft.name,
          owner: loaded.owner,
          agentPubkey: loaded.agentPubkey,
          sourcePresetD: loaded.sourcePresetD,
          agentEvents: agentEvents,
          personaEvents: personaEvents,
        )) {
      throw AgentSettingsException(agentNameConflictMessage(draft.name));
    }
  }

  final nowSeconds =
      (now ?? DateTime.now()).millisecondsSinceEpoch ~/
      Duration.millisecondsPerSecond;
  await relay.publish(
    kind: EventKind.managedAgent,
    content: patchedAgentContent(loaded.content, draft),
    tags: copyTags(head.tags),
    createdAt: nextCreatedAt(previous: head.createdAt, now: nowSeconds),
  );
}
