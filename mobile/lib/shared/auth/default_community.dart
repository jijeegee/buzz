import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../community/community.dart';
import '../relay/relay_provider.dart';
import '../relay/relay_socket.dart';
import 'google_key_backup.dart';
import 'token/relay_origin.dart';

/// Build-time environment, chosen with `--dart-define=KUZZ_ENV=prod|dev`.
enum KuzzEnvironment { prod, dev }

/// Stand-in origin for builds made without `KUZZ_RELAY_URL` (debug builds
/// and tests only: release and profile builds fail without it, see
/// [_KuzzRelayUrlDefine]). It never resolves, so such a build visibly
/// connects nowhere instead of to a guessed server.
const unsetServerOrigin = 'https://kuzz-relay-url-not-set.invalid';

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

/// This build's server address, `--dart-define=KUZZ_RELAY_URL=wss://...`
/// (or `config/prod.json`). Release and profile builds fail to compile
/// without it, like desktop's `build.rs`.
class _KuzzRelayUrlDefine {
  const _KuzzRelayUrlDefine(this.value)
    : assert(
        !(kReleaseMode || kProfileMode) || value != '',
        'KUZZ_RELAY_URL must be set for release and profile builds '
        '(the server this app connects to, e.g. wss://relay.example.com).',
      );

  final String value;
}

const _kuzzRelayUrl = _KuzzRelayUrlDefine(
  String.fromEnvironment('KUZZ_RELAY_URL'),
);

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
/// - [KuzzEnvironment.prod]: the build's [relayUrlDefine] (`KUZZ_RELAY_URL`),
///   which must be secure (`wss`/`https`); [keyBackupOriginDefine] is
///   ignored;
/// - [KuzzEnvironment.dev]: an explicit [keyBackupOriginDefine]
///   (`KUZZ_KEY_BACKUP_ORIGIN`), else [relayUrlDefine];
/// - with neither set (debug builds and tests only), [unsetServerOrigin];
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
  final defines = environment == KuzzEnvironment.dev
      ? [keyBackupOriginDefine, relayUrlDefine]
      : [relayUrlDefine];
  for (final define in defines) {
    if (define.trim().isEmpty) continue;
    final origin = normalizeRelayOrigin(define);
    if (environment == KuzzEnvironment.prod && !origin.startsWith('https://')) {
      throw ArgumentError.value(
        define,
        'KUZZ_RELAY_URL',
        'must be wss:// or https:// for prod builds',
      );
    }
    return origin;
  }
  return unsetServerOrigin;
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
/// same server, but a secure origin never matches `ws`/`http`. Prod origins
/// are always `https`, so only `wss`/`https` reach them; an insecure origin
/// exists only as an explicit dev override (see [builtInServerOrigin]).
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
  keyBackupOriginDefine: const String.fromEnvironment('KUZZ_KEY_BACKUP_ORIGIN'),
  relayUrlDefine: _kuzzRelayUrl.value,
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

/// A `connect_url` the built-in server [builtInOrigin] gave an account (its
/// home region, another address of the same service), or `null` when it is
/// unusable or just the built-in server again: a WebSocket URL with the same
/// security as the built-in server, so prod only takes `wss`.
String? acceptConnectUrl(String? url, String? builtInOrigin) {
  final trimmed = url?.trim() ?? '';
  final scheme = trimmed.split('://').first.toLowerCase();
  if (builtInOrigin == null || (scheme != 'wss' && scheme != 'ws')) {
    return null;
  }
  final given = _relayOrigin(trimmed);
  final builtIn = _relayOrigin(builtInOrigin);
  if (given == null || builtIn == null || given == builtIn) return null;
  final secure = given.startsWith('https://');
  return secure == builtIn.startsWith('https://') ? trimmed : null;
}

/// Addresses the built-in server gave each account (keyed by principal),
/// learned from `/auth/me` in this run.
final Map<String, String> _serverGivenConnectUrls = {};

/// Record what [acceptConnectUrl] made of the `connect_url` `/auth/me`
/// returned for [principal]; `null` forgets the account's address.
void rememberConnectUrl(String principal, String? acceptedUrl) {
  if (acceptedUrl == null) {
    _serverGivenConnectUrls.remove(principal);
  } else {
    _serverGivenConnectUrls[principal] = acceptedUrl;
  }
}

/// The address the built-in server gave [principal], if any.
String? serverGivenConnectUrl(String? principal) =>
    principal == null ? null : _serverGivenConnectUrls[principal];

/// [allowConnection] for the relay WebSocket, which may also reach an address
/// the built-in server gave an account. Everything else (signing, sessions,
/// push, one-shot sockets) stays on the built-in server.
bool allowRelayConnection(String context, String url) {
  final origin = _relayOrigin(url);
  if (origin != null &&
      _serverGivenConnectUrls.values.any((given) {
        return _relayOrigin(given) == origin;
      })) {
    return true;
  }
  return allowConnection(context, url);
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
