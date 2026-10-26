import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/leave_with_agents.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';

const _me = 'me';

ChannelMember _member(String pubkey, String role) =>
    ChannelMember(pubkey: pubkey, role: role, joinedAt: DateTime(2025));

class _Submitted {
  final int kind;
  final List<List<String>> tags;

  const _Submitted(this.kind, this.tags);
}

class _ScriptedRelay implements SignedEventRelay {
  _ScriptedRelay(this.failures);

  /// Removal failures by target pubkey.
  final Map<String, Object> failures;
  final submitted = <_Submitted>[];

  @override
  String? get pubkey => _me;

  @override
  Future<NostrEvent> submit({
    required int kind,
    required String content,
    required List<List<String>> tags,
    int? createdAt,
    void Function(NostrEvent event)? onSigned,
    void Function(NostrEvent previous, NostrEvent event)? onResigned,
  }) async {
    submitted.add(_Submitted(kind, tags));
    final target = tags.firstWhere((tag) => tag.first == 'p')[1];
    if (failures[target] case final failure?) throw failure;
    return const NostrEvent(
      id: 'ack',
      pubkey: '',
      createdAt: 0,
      kind: 0,
      tags: [],
      content: '',
      sig: '',
    );
  }
}

class _LeaveRecordingActions extends ChannelActions {
  _LeaveRecordingActions(Ref ref, _ScriptedRelay relay)
    : super(
        ref: ref,
        session: ref.read(relaySessionProvider.notifier),
        signedEventRelay: relay,
        currentPubkey: _me,
      );

  final left = <String>[];

  @override
  Future<void> leaveChannel(String channelId) async => left.add(channelId);
}

void main() {
  group('ownedAgentPubkeys', () {
    test('keeps only members whose verified owner is me', () {
      expect(
        ownedAgentPubkeys(
          memberPubkeys: ['ME', 'Agent-A', 'agent-b', 'agent-c', 'person'],
          me: _me,
          agentOwners: {'agent-a': 'ME', 'agent-c': 'someone'},
          profiles: {
            'agent-b': const UserProfile(pubkey: 'agent-b', ownerPubkey: _me),
            'person': const UserProfile(pubkey: 'person'),
          },
        ),
        ['agent-a', 'agent-b'],
      );
    });

    test('is empty without a signed-in identity', () {
      expect(
        ownedAgentPubkeys(
          memberPubkeys: ['agent-a'],
          me: null,
          agentOwners: {'agent-a': _me},
          profiles: const {},
        ),
        isEmpty,
      );
    });
  });

  test('isSoleOwner mirrors the relay leave refusal', () {
    expect(isSoleOwner([_member(_me, 'owner')], _me), isTrue);
    expect(
      isSoleOwner([_member(_me, 'owner'), _member('b', 'owner')], _me),
      isFalse,
    );
    expect(
      isSoleOwner([_member(_me, 'admin'), _member('b', 'owner')], _me),
      isFalse,
    );
    expect(isSoleOwner([_member(_me, 'member')], _me), isFalse);
  });

  group('removeAgentsAndLeave', () {
    Future<(_ScriptedRelay, _LeaveRecordingActions)> run(
      Map<String, Object> failures,
    ) async {
      final relay = _ScriptedRelay(failures);
      late _LeaveRecordingActions actions;
      final container = ProviderContainer(retry: (_, _) => null);
      addTearDown(container.dispose);
      final provider = Provider<ChannelActions>(
        (ref) => actions = _LeaveRecordingActions(ref, relay),
      );
      container.read(provider);
      try {
        await actions.removeAgentsAndLeave(
          channelId: 'room',
          agentPubkeys: ['Agent-A', 'agent-b'],
        );
      } catch (_) {
        // Inspected by the caller through the recorded calls.
      }
      return (relay, actions);
    }

    test('removes each agent with kind 9001, then leaves', () async {
      final (relay, actions) = await run(const {});
      expect([for (final s in relay.submitted) s.kind], [9001, 9001]);
      expect(relay.submitted.first.tags, [
        ['h', 'room'],
        ['p', 'agent-a'],
      ]);
      expect(actions.left, ['room']);
    });

    test('an agent already gone does not stop the leave', () async {
      final (relay, actions) = await run({
        'agent-a': Exception(
          'Relay rejected event: error: member not found in channel x',
        ),
      });
      expect(relay.submitted, hasLength(2));
      expect(actions.left, ['room']);
    });

    test('any other refusal stops before leaving', () async {
      final (relay, actions) = await run({
        'agent-a': Exception('Relay rejected event: not authorized'),
      });
      expect(relay.submitted, hasLength(1));
      expect(actions.left, isEmpty);
    });
  });

  testWidgets('the leave dialog returns the chosen way to leave', (
    tester,
  ) async {
    LeaveChoice? choice;
    var closed = false;
    await tester.pumpWidget(
      MaterialApp(
        theme: AppTheme.light(),
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              closed = false;
              choice = await showLeaveConfirmation(
                context: context,
                title: 'Leave?',
                body: 'Body',
                agentCount: 2,
              );
              closed = true;
            },
            child: const Text('open'),
          ),
        ),
      ),
    );

    for (final (key, expected) in [
      ('leave-remove-agents', LeaveChoice.removeAgentsAndLeave),
      ('leave-only', LeaveChoice.leaveOnly),
      ('leave-cancel', null),
    ]) {
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
      expect(find.text('Leave and remove my 2 agents'), findsOneWidget);
      await tester.tap(find.byKey(ValueKey(key)));
      await tester.pumpAndSettle();
      expect(closed, isTrue);
      expect(choice, expected, reason: key);
    }
  });
}
