import 'package:buzz/shared/devices/agent_badge_widgets.dart';
import 'package:buzz/shared/devices/device_robot.dart';
import 'package:buzz/shared/devices/device_robot_icon.dart';
import 'package:buzz/shared/profile/user_cache_provider.dart';
import 'package:buzz/shared/profile/user_profile.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

final _owner = 'a' * 64;

class _FakeUserCache extends UserCacheNotifier {
  @override
  Map<String, UserProfile> build() => {
    _owner: UserProfile(pubkey: _owner, displayName: 'Akak'),
  };

  @override
  UserProfile? get(String pubkey) => state[pubkey];
}

Future<void> _pump(WidgetTester tester, AgentBadge? badge) {
  return tester.pumpWidget(
    ProviderScope(
      overrides: [userCacheProvider.overrideWith(_FakeUserCache.new)],
      child: MaterialApp(
        home: Center(
          child: AgentAvatarBadge(
            badge: badge,
            child: const SizedBox.square(dimension: 42),
          ),
        ),
      ),
    ),
  );
}

void main() {
  testWidgets('another account sees the owner mark, labelled with the owner', (
    tester,
  ) async {
    await _pump(tester, AgentOwnerBadge(_owner));

    expect(find.byKey(ValueKey('agent-owner-mark-$_owner')), findsOneWidget);
    expect(find.bySemanticsLabel('Agent owned by Akak'), findsOneWidget);
    expect(find.byType(DeviceRobotIcon), findsNothing);
  });

  testWidgets('the owner sees the device robot', (tester) async {
    await _pump(
      tester,
      AgentDeviceBadge(deviceRobotVariantFromTag('e8c41c31')!),
    );

    expect(
      find.byKey(const ValueKey('device-robot-badge-e8c41c31')),
      findsOneWidget,
    );
    expect(find.byType(OwnerAvatarMark), findsNothing);
  });

  testWidgets('no badge leaves the avatar alone', (tester) async {
    await _pump(tester, null);

    expect(find.byType(OwnerAvatarMark), findsNothing);
    expect(find.byType(DeviceRobotIcon), findsNothing);
  });
}
