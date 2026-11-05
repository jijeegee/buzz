import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/agents/agent_settings_provider.dart';
import 'package:kuzz/features/agents/external_agent_access.dart';
import 'package:kuzz/shared/relay/relay.dart';

final owner = 'a' * 64;
final agent = 'b' * 64;

NostrEvent record(
  String content, {
  int createdAt = 100,
  String? author,
  String? id,
}) => NostrEvent(
  id: id ?? 'c' * 64,
  pubkey: author ?? owner,
  createdAt: createdAt,
  kind: EventKind.managedAgent,
  tags: [
    ['d', agent],
  ],
  content: content,
  sig: '',
);

String content(String respondTo, [List<String>? allowlist]) => jsonEncode({
  'name': 'Hermes',
  'parallelism': 1,
  'respond_to': respondTo,
  'respond_to_allowlist': ?allowlist,
});

class _Relay implements AgentSettingsRelay {
  List<NostrEvent> stored = [];
  Object? readFailure;
  Object? publishFailure;
  final published = <({String content, List<List<String>> tags, int at})>[];

  @override
  Future<List<NostrEvent>> fetch(NostrFilter filter) async {
    final failure = readFailure;
    if (failure != null) throw failure;
    return stored;
  }

  @override
  Future<void> publish({
    required int kind,
    required String content,
    required List<List<String>> tags,
    required int createdAt,
  }) async {
    expect(kind, EventKind.managedAgent);
    published.add((content: content, tags: tags, at: createdAt));
    final failure = publishFailure;
    if (failure != null) throw failure;
  }
}

final now = DateTime.fromMillisecondsSinceEpoch(1000 * 1000, isUtc: true);

Future<void> narrow(_Relay relay) => narrowExternalAgentAccess(
  relay,
  owner: owner,
  agentPubkey: agent,
  now: now,
);

void main() {
  group('externalAccessOf', () {
    test('no record or a bad one is owner-only', () {
      for (final head in [
        null,
        record('not json'),
        record('{"name":"a"}'),
        record('{"respond_to":"all"}'),
        record(content('everyone')),
        record('{"name":"a","parallelism":1,"respond_to":"anyone","x":'),
      ]) {
        expect(externalAccessOf(head).mode, AgentAccessMode.ownerOnly);
      }
    });

    test('reads a record as the connector reads it', () {
      final person = 'd' * 64;
      expect(
        externalAccessOf(record('{"respond_to":"anyone"}')).mode,
        AgentAccessMode.anyone,
      );
      expect(
        externalAccessOf(
          record(
            jsonEncode({
              'respond_to': 'allowlist',
              'respond_to_allowlist': [person, 1, 'bad'],
            }),
          ),
        ).allowlist,
        [person],
      );
    });

    test('reads what the card saved', () {
      final person = 'd' * 64;
      expect(
        externalAccessOf(record(content('anyone'))).mode,
        AgentAccessMode.anyone,
      );
      final allowlist = externalAccessOf(
        record(content('allowlist', [person])),
      );
      expect(allowlist.mode, AgentAccessMode.allowlist);
      expect(allowlist.allowlist, [person]);
    });
  });

  test('content has the fields the connector and desktop read', () {
    final person = 'd' * 64;
    expect(
      jsonDecode(
        externalAccessContent(
          name: 'Hermes',
          mode: AgentAccessMode.allowlist,
          allowlist: [person],
        ),
      ),
      {
        'name': 'Hermes',
        'parallelism': 1,
        'respond_to': 'allowlist',
        'respond_to_allowlist': [person],
      },
    );
    final wide =
        jsonDecode(
              externalAccessContent(
                name: 'Hermes',
                mode: AgentAccessMode.anyone,
                allowlist: [person],
              ),
            )
            as Map<String, dynamic>;
    expect(wide['respond_to'], 'anyone');
    expect(wide.containsKey('respond_to_allowlist'), isFalse);
  });

  group('saveExternalAgentAccess', () {
    test('replaces the head even within its second', () async {
      final relay = _Relay()
        ..stored = [record(content('anyone'), createdAt: 5000)];
      await saveExternalAgentAccess(
        relay,
        owner: owner,
        agentPubkey: agent,
        name: 'Hermes',
        mode: AgentAccessMode.ownerOnly,
        now: now,
      );
      expect(relay.published.single.at, 5001);
      expect(relay.published.single.tags, [
        ['d', agent],
      ]);
    });

    test('a failed publish reaches the caller', () async {
      final relay = _Relay()..publishFailure = StateError('offline');
      await expectLater(
        saveExternalAgentAccess(
          relay,
          owner: owner,
          agentPubkey: agent,
          name: 'Hermes',
          mode: AgentAccessMode.anyone,
        ),
        throwsStateError,
      );
    });
  });

  group('narrowExternalAgentAccess', () {
    test('no record: nothing to narrow', () async {
      final relay = _Relay();
      await narrow(relay);
      expect(relay.published, isEmpty);
    });

    test('owner-only stays as it is', () async {
      final relay = _Relay()..stored = [record(content('owner-only'))];
      await narrow(relay);
      expect(relay.published, isEmpty);
    });

    test('a wider setting goes back to owner-only, keeping the name', () async {
      final relay = _Relay()
        ..stored = [
          record(content('allowlist', ['d' * 64])),
        ];
      await narrow(relay);
      final saved =
          jsonDecode(relay.published.single.content) as Map<String, dynamic>;
      expect(saved['respond_to'], 'owner-only');
      expect(saved['name'], 'Hermes');
      expect(saved.containsKey('respond_to_allowlist'), isFalse);
    });

    test('a mode that cannot be read counts as wider', () async {
      final relay = _Relay()..readFailure = StateError('offline');
      await narrow(relay);
      final saved =
          jsonDecode(relay.published.single.content) as Map<String, dynamic>;
      expect(saved['respond_to'], 'owner-only');
    });

    test('a failure never reaches the disconnect', () async {
      final relay = _Relay()
        ..stored = [record(content('anyone'))]
        ..publishFailure = StateError('offline');
      await narrow(relay);
      expect(relay.published, hasLength(1));
    });
  });
}
