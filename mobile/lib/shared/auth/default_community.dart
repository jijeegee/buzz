import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community.dart';
import '../relay/relay_provider.dart';
import '../relay/relay_socket.dart';
import 'google_key_backup.dart';
import 'token/relay_origin.dart';

/// Build-time environment, chosen with `--dart-define=KUZZ_ENV=prod|dev`.
enum KuzzEnvironment { prod, dev }

/// The server each environment connects to. This table is the one place the
/// address lives; server-pushed address changes are a separate, later
/// feature.
const kuzzServerOrigins = <KuzzEnvironment, String>{
  KuzzEnvironment.prod: 'https://jihobuzz.duckdns.org:8443',
  // Same server for now. When dev and prod are split, change only this dev
  // entry (e.g. to the developer PC's address); prod stays as is.
  KuzzEnvironment.dev: 'https://jihobuzz.duckdns.org:8443',
};

/// `KUZZ_ENV` value to environment: unset or `prod` is
/// [KuzzEnvironment.prod], `dev` is [KuzzEnvironment.dev]. Any other value
/// throws [ArgumentError]; for this build's own define that is a compile
/// error instead (see [_KuzzEnvDefine]).
KuzzEnvironment parseKuzzEnvironment(String value) => switch (value) {
  '' || 'prod' => KuzzEnvironment.prod,
  'dev' => KuzzEnvironment.dev,
  _ => throw ArgumentError.value(
    value,
    'KUZZ_ENV',
    'must be `prod` or `dev` (or unset for prod)',
  ),
};

/// This build's `KUZZ_ENV`, checked while compiling: the const assert fails
/// constant evaluation (debug, profile and release alike), so an unknown
/// value fails the build like desktop's `build.rs` and a typo can never
/// fall back to another server.
class _KuzzEnvDefine {
  const _KuzzEnvDefine(this.value)
    : assert(
        value == '' || value == 'prod' || value == 'dev',
        'KUZZ_ENV must be `prod` or `dev` (or unset for prod).',
      );

  final String value;
}

const _kuzzEnv = _KuzzEnvDefine(String.fromEnvironment('KUZZ_ENV'));

/// Copy shown when an invite targets any other server.
const differentServiceInviteMessage = 'This invite is for a different service.';

/// Error for any attempt to add, select or sign in to another server.
const differentServerMessage = 'This app only connects to its own service.';

/// Test-harness seam, same pattern as
/// `PendingDeepLinkNotifier.debugUriStreamOverride`. Only
/// `test/flutter_test_config.dart` sets it, so suites written against other
/// relay hosts keep exercising the shared code. App code never writes it.
@visibleForTesting
bool debugUnrestrictedServerForTests = false;

/// Pure rule `(environment, overrides) -> origin` for the built-in server:
///
/// - [KuzzEnvironment.prod]: its table origin; the dart-define overrides are
///   ignored;
/// - [KuzzEnvironment.dev]: an explicit [keyBackupOriginDefine]
///   (`BUZZ_KEY_BACKUP_ORIGIN`), else [relayUrlDefine] (`BUZZ_RELAY_URL`),
///   else its table origin;
/// - the test harness ([unrestrictedForTests]) gets `null`, which keeps the
///   multi-server upstream behaviour for suites using fake relay hosts.
///
/// The result is normalized with [normalizeRelayOrigin]. No screen can
/// change it in any environment.
String? builtInServerOrigin(
  KuzzEnvironment environment, {
  String keyBackupOriginDefine = '',
  String relayUrlDefine = '',
  bool unrestrictedForTests = false,
}) {
  if (unrestrictedForTests) return null;
  if (environment == KuzzEnvironment.dev) {
    for (final define in [keyBackupOriginDefine, relayUrlDefine]) {
      if (define.trim().isNotEmpty) return normalizeRelayOrigin(define);
    }
  }
  return normalizeRelayOrigin(kuzzServerOrigins[environment]!);
}

/// [normalizeRelayOrigin] of [url], or `null` when it is not a relay URL.
String? _relayOrigin(String url) {
  try {
    return normalizeRelayOrigin(url);
  } on FormatException {
    return null;
  }
}

/// Whether [url] is the built-in server [builtInOrigin]: same host, port and
/// security. `wss` and `https` (or `ws` and `http`) of one host:port are the
/// same server, but a secure origin never matches `ws`/`http`. The prod and
/// dev table origins are `https`, so only `wss`/`https` reach them; an
/// insecure origin exists only as an explicit dev override (see
/// [builtInServerOrigin]).
bool isBuiltInServerUrl(String url, String builtInOrigin) {
  final candidate = _relayOrigin(url);
  return candidate != null && candidate == _relayOrigin(builtInOrigin);
}

/// Whether [url] may be added, selected or connected. A `null`
/// [builtInOrigin] (test harness only) allows every server.
bool isServerAllowed(String url, {required String? builtInOrigin}) =>
    builtInOrigin == null || isBuiltInServerUrl(url, builtInOrigin);

/// This build's built-in server origin (see [builtInServerOrigin]), for code
/// without a provider ref (push lease background work).
String? appBuiltInServerOrigin() => builtInServerOrigin(
  parseKuzzEnvironment(_kuzzEnv.value),
  keyBackupOriginDefine: const String.fromEnvironment('BUZZ_KEY_BACKUP_ORIGIN'),
  relayUrlDefine: const String.fromEnvironment('BUZZ_RELAY_URL'),
  unrestrictedForTests: debugUnrestrictedServerForTests,
);

/// Connection-layer guard, checked where a socket is created: whether [url]
/// may be opened under this build's rule ([isServerAllowed] with
/// [appBuiltInServerOrigin]). A refusal is logged with the [context] and the
/// target host:port only.
bool allowConnection(String context, String url) {
  if (isServerAllowed(url, builtInOrigin: appBuiltInServerOrigin())) {
    return true;
  }
  final target =
      _relayOrigin(url)?.split('://').last ?? 'an unparseable address';
  debugPrint('[Kuzz] refused $context for $target: not the built-in server');
  return false;
}

/// The built-in server origin (see [builtInServerOrigin]). Tests override it
/// to switch the fixed-server rule on with a fake origin.
final defaultCommunityOriginProvider = Provider<String?>(
  (ref) => appBuiltInServerOrigin(),
);

/// The fixed-server gate. When true, every server-level control (switch,
/// add, edit, remove, manual address entry) is hidden and only the built-in
/// server can be used.
final hasBuiltInServerProvider = Provider<bool>(
  (ref) => ref.watch(defaultCommunityOriginProvider) != null,
);

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
              'Couldn’t sign in to this community. Check your connection '
              'or that you’re still a member, then try again.',
            );
          }
        } on KeyBackupException {
          rethrow;
        } catch (_) {
          throw const KeyBackupException(
            'Couldn’t connect to this community. Check your connection, '
            'then try again.',
          );
        } finally {
          socket.dispose();
        }
      };
    });
