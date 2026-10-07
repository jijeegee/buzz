import 'dart:convert';

import 'dart:ui' show Color;

import 'package:flutter/foundation.dart';

/// Per-device robot variants.
///
/// Every relay device (one Google sign-in on one computer or phone) gets a
/// deterministic robot: a colour and a silhouette derived from its device id.
/// A desktop that runs an agent stamps its device tag into the agent's signed
/// kind:0 profile ([hostDeviceField]), so the agent's owner can tell which
/// computer it runs on, and the device list shows the same robot per device.
///
/// Mirrors desktop `shared/lib/deviceRobot.ts` + `deviceRobotGeometry.ts`;
/// `test-fixtures/device-robots.json` pins identical results on both.

/// kind:0 content field carrying the host device tag of an agent.
const hostDeviceField = 'buzz_host_device';

/// Robot stroke colours, each ≥3:1 against both light and dark surfaces.
const deviceRobotColors = <Color>[
  Color(0xFFD93D42), // red
  Color(0xFFC2620A), // orange
  Color(0xFF2E8540), // green
  Color(0xFF0F8478), // teal
  Color(0xFF1F78C8), // blue
  Color(0xFF5A5FD8), // indigo
  Color(0xFF9150C8), // violet
  Color(0xFFC93A8A), // pink
];

/// Robot silhouettes, in fixture order.
const deviceRobotShapes = <String>[
  'classic',
  'dome',
  'boxy',
  'visor',
  'tall',
  'hex',
];

final _tagPattern = RegExp(r'^[0-9a-f]{8}$');

/// One device's robot.
@immutable
class DeviceRobotVariant {
  const DeviceRobotVariant._(this.tag, this.colorIndex, this.shapeIndex);

  /// Eight lowercase hex digits; also the published [hostDeviceField].
  final String tag;
  final int colorIndex;
  final int shapeIndex;

  Color get color => deviceRobotColors[colorIndex];
  String get shape => deviceRobotShapes[shapeIndex];

  @override
  bool operator ==(Object other) =>
      other is DeviceRobotVariant && other.tag == tag;

  @override
  int get hashCode => tag.hashCode;
}

/// FNV-1a (32-bit) of the trimmed, lowercased device id as 8 hex digits;
/// `null` for an empty id. Hashing publishes no raw device id.
String? deviceRobotTag(String? deviceId) {
  final normalized = deviceId?.trim().toLowerCase() ?? '';
  if (normalized.isEmpty) return null;
  var hash = 0x811c9dc5;
  for (final byte in utf8.encode(normalized)) {
    hash ^= byte;
    hash = (hash * 0x01000193) & 0xffffffff;
  }
  return hash.toRadixString(16).padLeft(8, '0');
}

/// The robot for a device tag; `null` when absent or malformed.
DeviceRobotVariant? deviceRobotVariantFromTag(String? tag) {
  if (tag == null || !_tagPattern.hasMatch(tag)) return null;
  final value = int.parse(tag, radix: 16);
  final colorIndex = value % deviceRobotColors.length;
  final shapeIndex =
      (value ~/ deviceRobotColors.length) % deviceRobotShapes.length;
  return DeviceRobotVariant._(tag, colorIndex, shapeIndex);
}

/// The robot for a relay device id (device list rows).
DeviceRobotVariant? deviceRobotVariantForDevice(String? deviceId) =>
    deviceRobotVariantFromTag(deviceRobotTag(deviceId));

/// The well-formed [hostDeviceField] of decoded kind:0 content, if any.
String? hostDeviceFromMetadata(Map<String, dynamic> metadata) {
  final value = metadata[hostDeviceField];
  return value is String && _tagPattern.hasMatch(value) ? value : null;
}

/// The robot an agent shows to [viewerPubkey]: only the agent's verified
/// NIP-OA owner sees the device robot, so another account's agent cannot pose
/// as one of the viewer's own computers. Everyone else gets `null`.
DeviceRobotVariant? agentDeviceRobotVariant({
  required String? hostDevice,
  required String? ownerPubkey,
  required String? viewerPubkey,
}) {
  final owner = ownerPubkey?.toLowerCase();
  final viewer = viewerPubkey?.toLowerCase();
  if (owner == null || viewer == null || owner.isEmpty || owner != viewer) {
    return null;
  }
  return deviceRobotVariantFromTag(hostDevice);
}

/// A drawing primitive on the 24×24 robot grid (stroke 2, round caps/joins).
@immutable
sealed class RobotPrimitive {
  const RobotPrimitive();

  /// Fixture form, compared against `test-fixtures/device-robots.json`.
  Map<String, Object> toJson();
}

class RobotRect extends RobotPrimitive {
  const RobotRect(this.x, this.y, this.w, this.h, this.r);
  final double x, y, w, h, r;
  @override
  Map<String, Object> toJson() => {
    'k': 'rect',
    'x': x,
    'y': y,
    'w': w,
    'h': h,
    'r': r,
  };
}

class RobotLine extends RobotPrimitive {
  const RobotLine(this.x1, this.y1, this.x2, this.y2);
  final double x1, y1, x2, y2;
  @override
  Map<String, Object> toJson() => {
    'k': 'line',
    'x1': x1,
    'y1': y1,
    'x2': x2,
    'y2': y2,
  };
}

/// Stroked ring.
class RobotCircle extends RobotPrimitive {
  const RobotCircle(this.cx, this.cy, this.r);
  final double cx, cy, r;
  @override
  Map<String, Object> toJson() => {'k': 'circle', 'cx': cx, 'cy': cy, 'r': r};
}

/// Filled dot.
class RobotDot extends RobotPrimitive {
  const RobotDot(this.cx, this.cy, this.r);
  final double cx, cy, r;
  @override
  Map<String, Object> toJson() => {'k': 'dot', 'cx': cx, 'cy': cy, 'r': r};
}

class RobotPoly extends RobotPrimitive {
  const RobotPoly(this.points);
  final List<double> points;
  @override
  Map<String, Object> toJson() => {'k': 'poly', 'points': points};
}

/// Silhouettes keyed by [deviceRobotShapes]; mirrors desktop geometry.
const deviceRobotGeometry = <String, List<RobotPrimitive>>{
  // Square head, single antenna, side ears, slit eyes.
  'classic': [
    RobotRect(4, 8, 16, 12, 2),
    RobotLine(12, 8, 12, 4),
    RobotDot(12, 3.5, 1.5),
    RobotLine(2, 14, 4, 14),
    RobotLine(20, 14, 22, 14),
    RobotLine(9, 13, 9, 15),
    RobotLine(15, 13, 15, 15),
  ],
  // Round head, two antennae, round eyes.
  'dome': [
    RobotRect(5, 8, 14, 13, 6.5),
    RobotLine(9, 8.5, 7, 4),
    RobotLine(15, 8.5, 17, 4),
    RobotDot(9.5, 14.5, 1.5),
    RobotDot(14.5, 14.5, 1.5),
  ],
  // Sharp box head, side bolts, flat eyes and mouth.
  'boxy': [
    RobotRect(5, 6, 14, 14, 0.5),
    RobotLine(2, 10, 2, 16),
    RobotLine(22, 10, 22, 16),
    RobotLine(8.5, 11, 10.5, 11),
    RobotLine(13.5, 11, 15.5, 11),
    RobotLine(9, 16, 15, 16),
  ],
  // Wide pill head, visor band, T antenna.
  'visor': [
    RobotRect(3, 9, 18, 11, 5.5),
    RobotLine(7.5, 14.5, 16.5, 14.5),
    RobotLine(12, 9, 12, 5),
    RobotLine(9.5, 5, 14.5, 5),
  ],
  // Tall head, tall eyes, ears, mouth.
  'tall': [
    RobotRect(6, 3, 12, 18, 3),
    RobotLine(10, 8.5, 10, 11.5),
    RobotLine(14, 8.5, 14, 11.5),
    RobotLine(10, 16, 14, 16),
    RobotLine(3, 12, 6, 12),
    RobotLine(18, 12, 21, 12),
  ],
  // Hexagon head, ring eyes.
  'hex': [
    RobotPoly([12, 3, 20, 7.5, 20, 16.5, 12, 21, 4, 16.5, 4, 7.5]),
    RobotCircle(9, 12, 1.5),
    RobotCircle(15, 12, 1.5),
    RobotLine(10.5, 16.5, 13.5, 16.5),
  ],
};
