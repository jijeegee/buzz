import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../auth/token/relay_origin.dart';
import '../auth/token/token_session.dart';
import '../auth/token/token_session_provider.dart';
import 'community.dart';

/// Ends a token community's relay session when the community is removed.
typedef CommunityTokenSessionEnder =
    Future<void> Function(Community community, {required bool deviceOnly});

/// How community removal ends a token session.
///
/// By default the device session is revoked on the relay (`/auth/logout`)
/// and then forgotten locally; a relay that cannot be reached throws
/// [TokenSessionException] and keeps everything (AGENTS.md rule 1).
///
/// With `deviceOnly` the stored refresh token is deleted without contacting
/// the relay. That is the explicit "remove from this device only" recovery
/// path: the device then stays listed on the account until it is signed out
/// from another device.
final communityTokenSessionEnderProvider = Provider<CommunityTokenSessionEnder>(
  (ref) {
    return (community, {required deviceOnly}) async {
      if (!community.tokenAuth) return;
      final String origin;
      try {
        origin = normalizeRelayOrigin(community.relayUrl);
      } on FormatException {
        return;
      }
      final controller = ref.read(tokenSessionControllerProvider(origin));
      if (deviceOnly) {
        // Through the controller's store queue: a rotation write in flight
        // must not land after the delete and resurrect the record.
        await controller.forgetOnDevice();
        // A fresh controller restores from the now-empty store.
        ref.invalidate(tokenSessionControllerProvider(origin));
        return;
      }
      // A controller that never restored holds no tokens yet and would only
      // forget locally, stranding the live device session on the relay.
      if (controller.state.status == TokenSessionStatus.restoring) {
        await controller.restore();
      }
      await controller.signOut();
    };
  },
);
