import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../auth/account/account_api.dart';
import '../auth/token/token_session_provider.dart';
import '../relay/relay.dart';
import 'device_robot.dart';

/// Owner-authored map of one relay device to the agents it runs.
///
/// Parameterized replaceable, one event per device: `["d", <device id>]`,
/// content `{"v":1,"agents":[<agent pubkey hex>, ...]}`. The relay serves it
/// only to its author, so only the agents' owner learns where they run.
/// Desktop publishes it; mobile only reads it.
const kindAgentHostDevices = EventKind.agentHostDevices;

/// Owner-authored robot choice for one relay device.
///
/// Parameterized replaceable, author-only like [kindAgentHostDevices]:
/// `["d", <device id>]`, content `{"v":1,"shape":<shape>,"color":<index>}`
/// with a shape from [deviceRobotShapes] and an index into
/// [deviceRobotColors].
const kindDeviceRobot = EventKind.deviceRobot;

final _agentPubkeyPattern = RegExp(r'^[0-9a-f]{64}$');

/// One well-formed kind:30180 event of [owner].
typedef _HostDevicesEvent = ({
  String deviceId,
  List<String> agents,
  int createdAt,
  String id,
});

_HostDevicesEvent? _parseHostDevicesEvent(NostrEvent event, String owner) {
  if (event.kind != kindAgentHostDevices) return null;
  if (event.pubkey.toLowerCase() != owner) return null;
  final deviceId = event.getTagValue('d')?.trim().toLowerCase() ?? '';
  if (deviceId.isEmpty) return null;
  final Object? decoded;
  try {
    decoded = jsonDecode(event.content);
  } on FormatException {
    return null;
  }
  if (decoded is! Map<String, dynamic> || decoded['v'] != 1) return null;
  final agents = decoded['agents'];
  if (agents is! List) return null;
  final pubkeys = <String>[];
  for (final agent in agents) {
    if (agent is! String) return null;
    final pubkey = agent.toLowerCase();
    if (!_agentPubkeyPattern.hasMatch(pubkey)) return null;
    pubkeys.add(pubkey);
  }
  return (
    deviceId: deviceId,
    agents: pubkeys,
    createdAt: event.createdAt,
    id: event.id,
  );
}

/// NIP-01 replaceable order: later `created_at`, then the lower id.
bool _isNewer(_HostDevicesEvent a, _HostDevicesEvent b) =>
    a.createdAt != b.createdAt
    ? a.createdAt > b.createdAt
    : a.id.compareTo(b.id) < 0;

/// The kind:30181 content choosing [robot].
String deviceRobotChoiceContent(DeviceRobotVariant robot) =>
    jsonEncode({'v': 1, 'shape': robot.shape, 'color': robot.colorIndex});

/// The robot a kind:30181 [content] chooses; `null` when malformed, an
/// unknown shape or an out-of-range colour.
DeviceRobotVariant? parseDeviceRobotChoice(String content) {
  final Object? decoded;
  try {
    decoded = jsonDecode(content);
  } on FormatException {
    return null;
  }
  if (decoded is! Map<String, dynamic> || decoded['v'] != 1) return null;
  final shape = decoded['shape'];
  final color = decoded['color'];
  if (shape is! String || color is! int) return null;
  return deviceRobotVariantFromIndices(
    colorIndex: color,
    shapeIndex: deviceRobotShapes.indexOf(shape),
  );
}

/// Device id → chosen robot from [owner]'s kind:30181 events: the newest
/// event per device wins, and a malformed newest choice means the default.
Map<String, DeviceRobotVariant> deviceRobotOverridesFromEvents(
  Iterable<NostrEvent> events, {
  required String owner,
}) {
  final ownerLower = owner.toLowerCase();
  final newest = <String, NostrEvent>{};
  for (final event in events) {
    if (event.kind != kindDeviceRobot) continue;
    if (event.pubkey.toLowerCase() != ownerLower) continue;
    final deviceId = event.getTagValue('d')?.trim().toLowerCase() ?? '';
    if (deviceId.isEmpty) continue;
    final current = newest[deviceId];
    if (current == null ||
        event.createdAt > current.createdAt ||
        (event.createdAt == current.createdAt &&
            event.id.compareTo(current.id) < 0)) {
      newest[deviceId] = event;
    }
  }
  return {
    for (final entry in newest.entries)
      entry.key: ?parseDeviceRobotChoice(entry.value.content),
  };
}

/// Agent pubkey → device id from [owner]'s kind:30180 events.
///
/// Only the newest event per device counts (replaceable), and an agent listed
/// by several devices runs on the device of the newest event listing it.
/// Events by anyone else and malformed events are ignored.
Map<String, String> agentHostDevicesFromEvents(
  Iterable<NostrEvent> events, {
  required String owner,
}) {
  final ownerLower = owner.toLowerCase();
  final newestByDevice = <String, _HostDevicesEvent>{};
  for (final event in events) {
    final parsed = _parseHostDevicesEvent(event, ownerLower);
    if (parsed == null) continue;
    final current = newestByDevice[parsed.deviceId];
    if (current == null || _isNewer(parsed, current)) {
      newestByDevice[parsed.deviceId] = parsed;
    }
  }
  final winners = <String, _HostDevicesEvent>{};
  for (final device in newestByDevice.values) {
    for (final agent in device.agents) {
      final current = winners[agent];
      if (current == null || _isNewer(device, current)) winners[agent] = device;
    }
  }
  return {for (final entry in winners.entries) entry.key: entry.value.deviceId};
}

/// The current user's own device events (kind:30180 agent host devices and
/// kind:30181 device robots), by event id. Live: one subscription while
/// connected; known events survive reconnects. [publishDeviceRobot] adds its
/// event right away, so the new robot shows without waiting for the relay.
class OwnDeviceEventsNotifier extends Notifier<Map<String, NostrEvent>> {
  /// Received events by id (a resubscription replays them).
  final Map<String, NostrEvent> _events = {};
  String? _owner;

  @override
  Map<String, NostrEvent> build() {
    final owner = ref.watch(myPubkeyProvider)?.toLowerCase();
    final connected = ref.watch(
      relaySessionProvider.select((s) => s.status == SessionStatus.connected),
    );
    if (owner != _owner) {
      _owner = owner;
      _events.clear();
    }
    if (owner == null || owner.isEmpty) return const {};

    var disposed = false;
    void Function()? unsubscribe;
    ref.onDispose(() {
      disposed = true;
      unsubscribe?.call();
    });
    if (connected) {
      unawaited(() async {
        try {
          final unsub = await ref.read(relaySessionProvider.notifier).subscribe(
            NostrFilter(
              kinds: const [kindAgentHostDevices, kindDeviceRobot],
              authors: [owner],
              limit: 200,
            ),
            (event) {
              if (disposed) return;
              _add(event);
            },
          );
          if (disposed) {
            unsub();
          } else {
            unsubscribe = unsub;
          }
        } catch (error) {
          debugPrint('[OwnDeviceEvents] subscription failed: $error');
        }
      }());
    }
    return Map.unmodifiable(_events);
  }

  void _add(NostrEvent event) {
    _events[event.id] = event;
    state = Map.unmodifiable(_events);
  }

  /// Publish the current user's robot for [deviceId] (kind:30181) and show
  /// it at once. Throws when the relay rejects it or is unreachable.
  Future<void> publishDeviceRobot(
    String deviceId,
    DeviceRobotVariant robot,
  ) async {
    final d = deviceId.trim().toLowerCase();
    // Same-second choices tie on created_at and the relay keeps the lower id;
    // stay strictly newer than the choice this one replaces.
    final previous = _events.values
        .where(
          (e) =>
              e.kind == kindDeviceRobot &&
              e.getTagValue('d')?.trim().toLowerCase() == d,
        )
        .fold<int>(
          0,
          (latest, e) => e.createdAt > latest ? e.createdAt : latest,
        );
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    final event = buildOutgoingEvent(
      ref.read(relayConfigProvider),
      kind: kindDeviceRobot,
      content: deviceRobotChoiceContent(robot),
      tags: [
        ['d', d],
      ],
      createdAt: now > previous ? now : previous + 1,
    );
    await ref.read(relaySessionProvider.notifier).publish(event);
    _add(event);
  }
}

final ownDeviceEventsProvider =
    NotifierProvider<OwnDeviceEventsNotifier, Map<String, NostrEvent>>(
      OwnDeviceEventsNotifier.new,
    );

/// The current user's agents → the relay device each runs on (lowercase
/// keys and values), from their own kind:30180 events.
final agentHostDevicesProvider = Provider<Map<String, String>>((ref) {
  final owner = ref.watch(myPubkeyProvider);
  if (owner == null || owner.isEmpty) return const {};
  return agentHostDevicesFromEvents(
    ref.watch(ownDeviceEventsProvider).values,
    owner: owner,
  );
});

/// The current user's chosen robot per device id (lowercase), from their own
/// kind:30181 events. Devices without a valid choice are absent: they use
/// their hashed default ([resolveDeviceRobot]).
final deviceRobotOverridesProvider = Provider<Map<String, DeviceRobotVariant>>((
  ref,
) {
  final owner = ref.watch(myPubkeyProvider);
  if (owner == null || owner.isEmpty) return const {};
  return deviceRobotOverridesFromEvents(
    ref.watch(ownDeviceEventsProvider).values,
    owner: owner,
  );
});

/// Relay device id (lowercase) → its name, from the token relay's device
/// list (`GET /auth/devices`); empty without a token relay or on failure.
final agentHostDeviceNamesProvider = Provider.autoDispose<Map<String, String>>((
  ref,
) {
  final origin = ref.watch(activeTokenOriginProvider);
  if (origin == null) return const {};
  final devices = ref.watch(accountDevicesProvider(origin)).value;
  return {
    for (final device in devices ?? const <AccountDevice>[])
      device.id.trim().toLowerCase(): device.name,
  };
});

/// Long-press text naming the computer an agent runs on.
String agentHostDeviceLabel(String? deviceName) =>
    deviceName == null || deviceName.trim().isEmpty
    ? 'Running on another device'
    : 'Running on ${deviceName.trim()}';
