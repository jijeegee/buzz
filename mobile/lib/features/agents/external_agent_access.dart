import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'agent_settings.dart';
import 'agent_settings_provider.dart';

/// Who a paired agent answers. The connector reads the owner-signed
/// kind:30177 with the agent's key as `d`, the record app agents publish; no
/// record or a bad one means owner-only. Mirrors desktop's
/// `auth/external_access.rs`.
enum AgentAccessMode {
  ownerOnly('owner-only'),
  allowlist('allowlist'),
  anyone('anyone');

  const AgentAccessMode(this.wire);
  final String wire;
}

class ExternalAgentAccess {
  const ExternalAgentAccess({required this.mode, this.allowlist = const []});

  final AgentAccessMode mode;
  final List<String> allowlist;

  static const ownerOnly = ExternalAgentAccess(mode: AgentAccessMode.ownerOnly);
}

/// The setting in [head], read as the connector reads it; none or a bad
/// record is owner-only.
ExternalAgentAccess externalAccessOf(NostrEvent? head) {
  final content = head == null ? null : decodeJsonObject(head.content);
  final allowlist = content?['respond_to_allowlist'];
  return switch (content?['respond_to']) {
    'anyone' => const ExternalAgentAccess(mode: AgentAccessMode.anyone),
    'allowlist' => ExternalAgentAccess(
      mode: AgentAccessMode.allowlist,
      allowlist: [
        if (allowlist is List)
          for (final person in allowlist.whereType<String>())
            if (_hex64.hasMatch(person)) person.toLowerCase(),
      ],
    ),
    _ => ExternalAgentAccess.ownerOnly,
  };
}

final _hex64 = RegExp(r'^[0-9a-fA-F]{64}$');

/// The record's content. Only an allowlist keeps its people.
String externalAccessContent({
  required String name,
  required AgentAccessMode mode,
  List<String> allowlist = const [],
}) => jsonEncode({
  'name': name,
  'parallelism': 1,
  'respond_to': mode.wire,
  if (mode == AgentAccessMode.allowlist) 'respond_to_allowlist': allowlist,
});

/// The account that can write access records. A token session has no key:
/// it sends drafts, which the connector refuses as unsigned.
final externalAccessWriterProvider = Provider<String?>((ref) {
  if (ref.watch(relayConfigProvider).tokenAuth) return null;
  return ref.watch(myPubkeyProvider)?.toLowerCase();
});

final externalAgentAccessProvider = FutureProvider.autoDispose
    .family<ExternalAgentAccess, String>((ref, agentPubkey) async {
      final owner = ref.watch(myPubkeyProvider)?.toLowerCase();
      if (owner == null || owner.isEmpty) return ExternalAgentAccess.ownerOnly;
      final head = await _readHead(
        ref.read(agentSettingsRelayProvider),
        owner,
        agentPubkey.toLowerCase(),
      );
      return externalAccessOf(head);
    });

Future<NostrEvent?> _readHead(
  AgentSettingsRelay relay,
  String owner,
  String agent,
) async {
  final events = await relay.fetch(
    NostrFilter(
      kinds: const [EventKind.managedAgent],
      authors: [owner],
      tags: {
        '#d': [agent],
      },
      limit: 10,
    ),
  );
  return newestByDTag(
    events,
    kind: EventKind.managedAgent,
    owner: owner,
  )[agent];
}

Future<void> _publish(
  AgentSettingsRelay relay,
  String agent,
  String content,
  NostrEvent? head,
  DateTime? now,
) {
  final nowSeconds =
      (now ?? DateTime.now()).millisecondsSinceEpoch ~/
      Duration.millisecondsPerSecond;
  return relay.publish(
    kind: EventKind.managedAgent,
    content: content,
    tags: [
      ['d', agent],
    ],
    createdAt: head == null
        ? nowSeconds
        : nextCreatedAt(previous: head.createdAt, now: nowSeconds),
  );
}

/// Saved only once this completes: the relay accepted the record.
Future<void> saveExternalAgentAccess(
  AgentSettingsRelay relay, {
  required String owner,
  required String agentPubkey,
  required String name,
  required AgentAccessMode mode,
  List<String> allowlist = const [],
  DateTime? now,
}) async {
  final agent = agentPubkey.toLowerCase();
  final head = await _readHead(relay, owner, agent);
  await _publish(
    relay,
    agent,
    externalAccessContent(name: name, mode: mode, allowlist: allowlist),
    head,
    now,
  );
}

/// Publish owner-only for an agent being disconnected unless it is known to
/// be owner-only, so pairing the same key again can't bring back a wider
/// setting. A mode that can't be read counts as wider. Logged, never in the
/// way of a disconnect, which must work when this can't.
Future<void> narrowExternalAgentAccess(
  AgentSettingsRelay relay, {
  required String owner,
  required String agentPubkey,
  DateTime? now,
}) async {
  final agent = agentPubkey.toLowerCase();
  try {
    NostrEvent? head;
    var known = true;
    try {
      head = await _readHead(relay, owner, agent);
    } catch (_) {
      known = false;
    }
    if (known) {
      if (head == null) return;
      if (externalAccessOf(head).mode == AgentAccessMode.ownerOnly) return;
    }
    final name = head == null ? null : decodeJsonObject(head.content)?['name'];
    await _publish(
      relay,
      agent,
      externalAccessContent(
        name: name is String ? name : 'Agent',
        mode: AgentAccessMode.ownerOnly,
      ),
      head,
      now,
    );
  } catch (error) {
    debugPrint('[ExternalAgentAccess] narrowing failed: $error');
  }
}
