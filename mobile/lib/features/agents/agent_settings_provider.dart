import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
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

/// The agent has no owner definition the phone can edit (built-in and team
/// agents, or one whose definition is not on the relay).
class AgentSettingsReadOnly extends AgentSettingsState {
  const AgentSettingsReadOnly({required this.name});
  final String name;
}

/// The agent's owner definition (kind:30175), ready to edit.
class AgentSettingsEditable extends AgentSettingsState {
  const AgentSettingsEditable({
    required this.owner,
    required this.agentPubkey,
    required this.personaD,
    required this.event,
    required this.content,
    required this.agentName,
  });

  /// The tapped agent's own name (its kind:30177 projection).
  final String agentName;

  /// Renaming the definition renames only agents that carry its name, so a
  /// differently named agent (e.g. "Fizz 2") keeps its name here.
  bool get nameEditable => agentName == draft.name;

  final String owner;
  final String agentPubkey;
  final String personaD;

  /// The definition event as loaded; a save refuses if it was replaced.
  final NostrEvent event;
  final Map<String, dynamic> content;

  AgentSettingsDraft get draft => AgentSettingsDraft.fromContent(content);
}

/// A save the user has to fix or retry; [message] is shown as is.
class AgentSettingsException implements Exception {
  const AgentSettingsException(this.message);
  final String message;

  @override
  String toString() => message;
}

String get agentSettingsReadOnlyMessage => appL10n.agentsSettingsReadOnly;

String get agentSettingsNameLockedMessage => appL10n.agentsSettingsNameLocked;

String get agentSettingsChangedMessage => appL10n.agentsSettingsChanged;

/// Loads the editable definition of the agent with this pubkey, or
/// [AgentSettingsReadOnly] named after its kind:30177 projection (falls back
/// to [AgentSettingsArgs.fallbackName]).
final agentSettingsProvider = FutureProvider.autoDispose
    .family<AgentSettingsState, AgentSettingsArgs>((ref, args) async {
      final owner = ref.watch(myPubkeyProvider)?.toLowerCase();
      if (owner == null || owner.isEmpty) {
        return AgentSettingsReadOnly(name: args.fallbackName);
      }
      final relay = ref.read(agentSettingsRelayProvider);
      final agentPubkey = args.agentPubkey.toLowerCase();
      final agentEvents = await relay.fetch(
        NostrFilter(
          kinds: const [EventKind.managedAgent],
          authors: [owner],
          tags: {
            '#d': [agentPubkey],
          },
          limit: 10,
        ),
      );
      final agentEvent = newestByDTag(
        agentEvents,
        kind: EventKind.managedAgent,
        owner: owner,
      )[agentPubkey];
      final agentContent = agentEvent == null
          ? null
          : decodeJsonObject(agentEvent.content);
      final agentName = agentContent?['name'];
      final name = agentName is String && agentName.trim().isNotEmpty
          ? agentName.trim()
          : args.fallbackName;
      final personaId = agentEvent == null
          ? null
          : managedAgentPersonaId(agentEvent.content);
      if (personaId == null) return AgentSettingsReadOnly(name: name);
      final personaD = personaDTag(personaId);
      final personaEvent = await _fetchPersona(relay, owner, personaD);
      final content = personaEvent == null
          ? null
          : decodeJsonObject(personaEvent.content);
      if (personaEvent == null || content == null) {
        return AgentSettingsReadOnly(name: name);
      }
      return AgentSettingsEditable(
        owner: owner,
        agentPubkey: agentPubkey,
        personaD: personaD,
        event: personaEvent,
        content: content,
        agentName: agentName is String ? agentName : name,
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

Future<NostrEvent?> _fetchPersona(
  AgentSettingsRelay relay,
  String owner,
  String personaD,
) async {
  final events = await relay.fetch(
    NostrFilter(
      kinds: const [EventKind.persona],
      authors: [owner],
      tags: {
        '#d': [personaD],
      },
      limit: 10,
    ),
  );
  return newestEvent(
    events.where(
      (e) =>
          e.kind == EventKind.persona &&
          e.pubkey.toLowerCase() == owner &&
          e.getTagValue('d') == personaD,
    ),
  );
}

/// Publish [draft] over [loaded]'s definition. Throws
/// [AgentSettingsException] with a user-facing message when the draft is
/// invalid, the name is taken, or the definition changed since it loaded.
Future<void> saveAgentSettings(
  AgentSettingsRelay relay,
  AgentSettingsEditable loaded,
  AgentSettingsDraft draft, {
  DateTime? now,
}) async {
  if (!loaded.nameEditable && draft.name != loaded.draft.name) {
    throw AgentSettingsException(agentSettingsNameLockedMessage);
  }
  final invalid = validateAgentSettings(draft);
  if (invalid != null) throw AgentSettingsException(invalid);

  final head = await _fetchPersona(relay, loaded.owner, loaded.personaD);
  if (head == null || head.id != loaded.event.id) {
    throw AgentSettingsException(agentSettingsChangedMessage);
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
          previousName: loaded.draft.name,
          owner: loaded.owner,
          agentPubkey: loaded.agentPubkey,
          personaD: loaded.personaD,
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
    kind: EventKind.persona,
    content: patchedPersonaContent(loaded.content, draft),
    tags: copyTags(head.tags),
    createdAt: nextCreatedAt(previous: head.createdAt, now: nowSeconds),
  );
}
