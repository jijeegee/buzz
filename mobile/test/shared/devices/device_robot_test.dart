import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:buzz/shared/devices/device_robot.dart';
import 'package:buzz/shared/devices/device_robot_icon.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

/// Shared with desktop (`deviceRobot.test.mjs`) and Tauri (`device_robot.rs`).
final Map<String, dynamic> _golden =
    jsonDecode(File('../test-fixtures/device-robots.json').readAsStringSync())
        as Map<String, dynamic>;

/// Numbers as doubles so `4` (fixture JSON) equals `4.0` (Dart geometry).
Object? _normalize(Object? value) => switch (value) {
  final num n => n.toDouble(),
  final List<dynamic> list => [for (final item in list) _normalize(item)],
  final Map<dynamic, dynamic> map => {
    for (final entry in map.entries) '${entry.key}': _normalize(entry.value),
  },
  _ => value,
};

String _hex(Color color) {
  final rgb = (color.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0');
  return '#${rgb.toUpperCase()}';
}

String _tagFor(int value) => value.toRadixString(16).padLeft(8, '0');

void main() {
  test('palette, silhouettes and geometry match the shared fixture', () {
    expect(deviceRobotColors.map(_hex).toList(), _golden['colors']);
    expect(deviceRobotShapes, _golden['shapes']);
    expect(
      _normalize({
        for (final entry in deviceRobotGeometry.entries)
          entry.key: [for (final shape in entry.value) shape.toJson()],
      }),
      _normalize(_golden['geometry']),
    );
  });

  test('device ids map to the shared tags and variants', () {
    final devices = (_golden['devices'] as List<dynamic>)
        .cast<Map<String, dynamic>>();
    expect(devices, isNotEmpty);
    for (final vector in devices) {
      final id = vector['deviceId'] as String;
      expect(deviceRobotTag(id), vector['tag'], reason: id);
      final variant = deviceRobotVariantForDevice(id)!;
      expect(variant.tag, vector['tag']);
      expect(variant.colorIndex, vector['colorIndex'], reason: id);
      expect(variant.shapeIndex, vector['shapeIndex'], reason: id);
    }
    final extra = (_golden['extraTags'] as List<dynamic>)
        .cast<Map<String, dynamic>>();
    for (final vector in extra) {
      final variant = deviceRobotVariantFromTag(vector['tag'] as String)!;
      expect(variant.colorIndex, vector['colorIndex']);
      expect(variant.shapeIndex, vector['shapeIndex']);
    }
  });

  test('unknown devices and malformed tags fall back to the default', () {
    final emptyIds = (_golden['emptyDeviceIds'] as List<dynamic>)
        .cast<String>();
    for (final id in <String?>[...emptyIds, null]) {
      expect(deviceRobotTag(id), isNull);
      expect(deviceRobotVariantForDevice(id), isNull);
    }
    final invalid = (_golden['invalidTags'] as List<dynamic>).cast<String>();
    for (final tag in invalid) {
      expect(deviceRobotVariantFromTag(tag), isNull, reason: tag);
      expect(hostDeviceFromMetadata({hostDeviceField: tag}), isNull);
    }
    expect(hostDeviceFromMetadata({hostDeviceField: 7}), isNull);
    expect(hostDeviceFromMetadata({hostDeviceField: 'e8c41c31'}), 'e8c41c31');
  });

  test('agent badge: device robot for the owner, owner mark for others', () {
    final owner = 'a' * 64;
    const tag = 'e8c41c31';
    final mine = agentBadge(
      hostDevice: tag,
      ownerPubkey: owner,
      viewerPubkey: owner.toUpperCase(),
    );
    expect(mine, isA<AgentDeviceBadge>());
    expect((mine! as AgentDeviceBadge).variant.tag, tag);
    final theirs = agentBadge(
      hostDevice: tag,
      ownerPubkey: owner.toUpperCase(),
      viewerPubkey: 'b' * 64,
    );
    expect(theirs, isA<AgentOwnerBadge>());
    expect((theirs! as AgentOwnerBadge).ownerPubkey, owner);
    expect(
      agentBadge(hostDevice: null, ownerPubkey: 'c' * 64, viewerPubkey: owner),
      isA<AgentOwnerBadge>(),
      reason: 'others see the owner even without a host device',
    );
    expect(
      agentBadge(hostDevice: tag, ownerPubkey: null, viewerPubkey: owner),
      isNull,
    );
    expect(
      agentBadge(hostDevice: null, ownerPubkey: owner, viewerPubkey: owner),
      isNull,
    );
    expect(
      agentBadge(hostDevice: tag, ownerPubkey: owner, viewerPubkey: null),
      isNull,
    );
  });

  testWidgets('icon paints the variant, or the default bot when unknown', (
    tester,
  ) async {
    final variant = deviceRobotVariantFromTag('e8c41c31')!;
    await tester.pumpWidget(
      MaterialApp(
        home: Row(
          children: [
            DeviceRobotIcon(variant: variant, semanticLabel: 'Device robot'),
            const DeviceRobotIcon(variant: null),
          ],
        ),
      ),
    );
    expect(find.byKey(const ValueKey('device-robot-e8c41c31')), findsOneWidget);
    expect(find.byType(Icon), findsOneWidget);
    expect(find.bySemanticsLabel('Device robot'), findsOneWidget);
  });

  testWidgets('avatar badge appears only for a known device', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Row(
          children: [
            DeviceRobotAvatarBadge(
              robot: deviceRobotVariantFromTag('e8c41c31'),
              child: const SizedBox.square(dimension: 42),
            ),
            const DeviceRobotAvatarBadge(
              robot: null,
              child: SizedBox.square(dimension: 42),
            ),
          ],
        ),
      ),
    );
    expect(
      find.byKey(const ValueKey('device-robot-badge-e8c41c31')),
      findsOneWidget,
    );
    expect(find.byType(DeviceRobotIcon), findsOneWidget);
  });

  testWidgets('every variant renders on light and dark surfaces', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(2 * (8 * 40 + 16), 6 * 40 + 16);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    Widget grid(Color background) => Container(
      color: background,
      padding: const EdgeInsets.all(8),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var shape = 0; shape < deviceRobotShapes.length; shape++)
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (var color = 0; color < deviceRobotColors.length; color++)
                  Padding(
                    padding: const EdgeInsets.all(4),
                    child: DeviceRobotIcon(
                      variant: deviceRobotVariantFromTag(
                        _tagFor(shape * deviceRobotColors.length + color),
                      ),
                      size: 32,
                    ),
                  ),
              ],
            ),
        ],
      ),
    );
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Align(
          alignment: Alignment.topLeft,
          child: RepaintBoundary(
            key: const Key('robots'),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                grid(const Color(0xFFFFFFFF)),
                grid(const Color(0xFF1C1C1F)),
              ],
            ),
          ),
        ),
      ),
    );
    // Every colour/shape pair is distinct and painted.
    final tags = {
      for (var i = 0; i < 48; i++) deviceRobotVariantFromTag(_tagFor(i))!,
    };
    expect(
      tags.map((v) => '${v.colorIndex}/${v.shapeIndex}').toSet(),
      hasLength(48),
    );
    expect(find.byType(DeviceRobotIcon), findsNWidgets(96));
    // Opt-in screenshot for reviews: DEVICE_ROBOT_SCREENSHOT=<png path>.
    // Not a golden: anti-aliasing differs across CI hosts.
    final screenshot = Platform.environment['DEVICE_ROBOT_SCREENSHOT'];
    if (screenshot != null) {
      final boundary = tester.renderObject<RenderRepaintBoundary>(
        find.byKey(const Key('robots')),
      );
      await tester.runAsync(() async {
        final image = await boundary.toImage(pixelRatio: 2);
        final png = await image.toByteData(format: ui.ImageByteFormat.png);
        File(screenshot).writeAsBytesSync(png!.buffer.asUint8List());
      });
    }
  });
}
