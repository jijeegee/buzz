import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'agent_settings.dart';
import 'agent_settings_provider.dart';

/// One agent definition in a team, and the owner's agents made from it.
class MyTeamMember {
  const MyTeamMember({
    required this.personaD,
    required this.name,
    required this.agentPubkeys,
  });

  /// The definition's kind:30175 `d` tag.
  final String personaD;

  /// The definition's name; `null` when the definition is not on the relay.
  final String? name;

  /// Lowercase pubkeys of the owner's agents that use this definition.
  final List<String> agentPubkeys;
}

/// One of the owner's agent teams (kind:30176), as desktop publishes it.
class MyTeam {
  const MyTeam({
    required this.id,
    required this.name,
    required this.description,
    required this.members,
  });

  final String id;
  final String name;
  final String? description;
  final List<MyTeamMember> members;
}

/// The owner's teams from their kind:30176 [teamEvents], with each member's
/// name from its kind:30175 definition and its agents from kind:30177
/// (`persona_id`). Built-in teams are never published, so they are not here.
/// Sorted by name.
List<MyTeam> myTeamsFrom({
  required String owner,
  required List<NostrEvent> teamEvents,
  required List<NostrEvent> personaEvents,
  required List<NostrEvent> agentEvents,
}) {
  final personaNames = <String, String?>{};
  for (final MapEntry(:key, :value) in newestByDTag(
    personaEvents,
    kind: EventKind.persona,
    owner: owner,
  ).entries) {
    final name = decodeJsonObject(value.content)?['display_name'];
    personaNames[key] = name is String && name.trim().isNotEmpty
        ? name.trim()
        : null;
  }
  final agentsByPersona = <String, List<String>>{};
  for (final MapEntry(:key, :value) in newestByDTag(
    agentEvents,
    kind: EventKind.managedAgent,
    owner: owner,
  ).entries) {
    final personaId = managedAgentPersonaId(value.content);
    if (personaId == null) continue;
    (agentsByPersona[personaDTag(personaId)] ??= []).add(key.toLowerCase());
  }
  final teams = <MyTeam>[];
  for (final MapEntry(:key, :value) in newestByDTag(
    teamEvents,
    kind: EventKind.team,
    owner: owner,
  ).entries) {
    final content = decodeJsonObject(value.content);
    final name = content?['name'];
    if (name is! String || name.trim().isEmpty) continue;
    final description = content?['description'];
    final personaIds = content?['persona_ids'];
    final seen = <String>{};
    final members = <MyTeamMember>[
      if (personaIds is List)
        for (final id in personaIds)
          if (id is String && id.trim().isNotEmpty && seen.add(personaDTag(id)))
            MyTeamMember(
              personaD: personaDTag(id),
              name: personaNames[personaDTag(id)] ?? _builtinName(id),
              agentPubkeys: [...?agentsByPersona[personaDTag(id)]]..sort(),
            ),
    ];
    teams.add(
      MyTeam(
        id: key,
        name: name.trim(),
        description: description is String && description.trim().isNotEmpty
            ? description.trim()
            : null,
        members: members,
      ),
    );
  }
  teams.sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
  return teams;
}

/// Desktop's built-in definitions (`builtin:fizz`) are never published, so
/// their name comes from the id.
String? _builtinName(String personaId) {
  const prefix = 'builtin:';
  if (!personaId.startsWith(prefix)) return null;
  final rest = personaId.substring(prefix.length).trim();
  if (rest.isEmpty) return null;
  return rest[0].toUpperCase() + rest.substring(1);
}

/// The signed-in owner's agent teams; empty when signed out.
final myTeamsProvider = FutureProvider.autoDispose<List<MyTeam>>((ref) async {
  final owner = ref.watch(myPubkeyProvider)?.toLowerCase();
  if (owner == null || owner.isEmpty) return const [];
  // Like the agent directory: wait for a connection, and refetch after
  // every reconnect (a fetch sent while disconnected is dropped).
  final status = ref.watch(relaySessionProvider.select((s) => s.status));
  if (status != SessionStatus.connected) return const [];
  final relay = ref.read(agentSettingsRelayProvider);
  final results = await Future.wait([
    relay.fetch(
      NostrFilter(kinds: const [EventKind.team], authors: [owner], limit: 200),
    ),
    relay.fetch(
      NostrFilter(
        kinds: const [EventKind.persona],
        authors: [owner],
        limit: 500,
      ),
    ),
    relay.fetch(
      NostrFilter(
        kinds: const [EventKind.managedAgent],
        authors: [owner],
        limit: 500,
      ),
    ),
  ]);
  return myTeamsFrom(
    owner: owner,
    teamEvents: results[0],
    personaEvents: results[1],
    agentEvents: results[2],
  );
});
