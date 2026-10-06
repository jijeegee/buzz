import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community.dart';
import '../relay/relay_provider.dart';
import '../relay/relay_socket.dart';
import 'google_key_backup.dart';
import 'token/relay_origin.dart';

/// Explicit deployment configuration, independent of any selected community.
/// Generic clients have no implicit custody host. Dev recipes supply this.
final defaultCommunityOriginProvider = Provider<String?>((ref) {
  const configured = String.fromEnvironment('BUZZ_KEY_BACKUP_ORIGIN');
  return configured.isEmpty ? null : normalizeRelayOrigin(configured);
});

/// Completes the existing signed NIP-42 handshake before first-run enrollment.
/// The normal persistent relay session takes over after community persistence.
final signedCommunityAdmissionProvider =
    Provider<Future<void> Function(Community)>((ref) {
      return (community) async {
        final config = RelayConfig(
          baseUrl: community.relayUrl,
          nsec: community.nsec,
        );
        final socket = RelaySocket(
          wsUrl: config.wsUrl,
          nsec: config.nsec,
          onMessage: (_) {},
          onConnected: () {},
          onDisconnected: (_) {},
        );
        try {
          await socket.connect().timeout(const Duration(seconds: 15));
          if (socket.state != SocketState.connected) {
            throw const KeyBackupException(
              'Could not authenticate to this community. Your key backup is '
              'safe. Check your connection or membership, then retry.',
            );
          }
        } on KeyBackupException {
          rethrow;
        } catch (_) {
          throw const KeyBackupException(
            'Could not connect to this community. Your key backup is safe. '
            'Check your connection, then retry.',
          );
        } finally {
          socket.dispose();
        }
      };
    });
