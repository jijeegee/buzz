import 'package:flutter/foundation.dart';

import '../auth/default_community.dart';
import '../auth/token/relay_origin.dart';

/// Storage scope of the prod service. Every persistent key that used to embed
/// the server address embeds this instead, so a domain change or server move
/// keeps sign-in, settings and drafts.
const prodServiceScope = 'kuzz';

/// Storage scope of the dev service (`KUZZ_ENV=dev`).
const devServiceScope = 'kuzz-dev';

/// The addresses this build knows as its own service.
///
/// The service is identified by [scope]; its addresses are only used to
/// connect. [currentOrigin] is where the build connects today, [origins] is
/// every address the service has had (current and previous), normalized with
/// [normalizeRelayOrigin].
@immutable
class ServiceAddresses {
  ServiceAddresses({
    required this.scope,
    required this.currentOrigin,
    Iterable<String> otherOrigins = const [],
  }) : origins = List.unmodifiable(
         {
           ?currentOrigin,
           for (final url in otherOrigins) ?_originOrNull(url),
         }.toList()..sort((a, b) => b.length.compareTo(a.length)),
       );

  /// The service of a build with these defines: `KUZZ_ENV` picks the
  /// [scope], the built-in server ([builtInServerOrigin]) is the current
  /// address, and `KUZZ_RELAY_URL`, `KUZZ_KEY_BACKUP_ORIGIN` and the
  /// comma-separated `KUZZ_PREVIOUS_RELAY_URLS` are all its addresses.
  /// Without any address define (tests, bare debug builds) nothing is a
  /// service address.
  factory ServiceAddresses.fromDefines({
    required String environment,
    String relayUrl = '',
    String keyBackupOrigin = '',
    String previousRelayUrls = '',
  }) {
    final env = parseKuzzEnvironment(environment);
    final scope = env == KuzzEnvironment.dev
        ? devServiceScope
        : prodServiceScope;
    String? current;
    if (relayUrl.trim().isNotEmpty || keyBackupOrigin.trim().isNotEmpty) {
      try {
        current = builtInServerOrigin(
          env,
          keyBackupOriginDefine: keyBackupOrigin,
          relayUrlDefine: relayUrl,
        );
      } on ArgumentError {
        current = null;
      } on FormatException {
        current = null;
      }
    }
    return ServiceAddresses(
      scope: scope,
      currentOrigin: current == unsetServerOrigin ? null : current,
      otherOrigins: [
        relayUrl,
        keyBackupOrigin,
        ...previousRelayUrls.split(','),
      ].where((url) => url.trim().isNotEmpty),
    );
  }

  final String scope;

  /// Normalized origin this build connects to, or `null` without one.
  final String? currentOrigin;

  /// Every normalized service origin, longest first.
  final List<String> origins;

  /// Whether [url] (any http/https/ws/wss URL or bare host, any path) is one
  /// of this service's addresses. Default ports compare equal
  /// (`wss://host` is `https://host:443`). Unparseable input is `false`.
  bool isServiceAddress(String url) {
    final origin = _originOrNull(url);
    return origin != null && origins.contains(origin);
  }

  /// The persistent-storage scope for a value that used to be keyed by
  /// [url]: [scope] for a service address, otherwise [url] unchanged.
  String storageScopeFor(String url) => isServiceAddress(url) ? scope : url;

  /// [url] moved onto [currentOrigin] when it is a service address: scheme,
  /// host and port are replaced (`ws`/`wss` stay websocket, `http`/`https`
  /// stay HTTP) and the path, query and fragment are kept byte for byte.
  /// Any other URL is returned unchanged.
  String currentServiceUrl(String url) {
    final current = currentOrigin;
    if (current == null || !isServiceAddress(url)) return url;
    if (_originOrNull(url) == current) return url;
    final trimmed = url.trim();
    final schemeEnd = trimmed.indexOf('://');
    final rest = schemeEnd < 0 ? trimmed : trimmed.substring(schemeEnd + 3);
    final authorityEnd = rest.indexOf(RegExp(r'[/?#]'));
    final tail = authorityEnd < 0 ? '' : rest.substring(authorityEnd);
    final currentAuthority = current.substring(current.indexOf('://') + 3);
    if (schemeEnd < 0) return '$currentAuthority$tail';
    final secure = current.startsWith('https://');
    final scheme = trimmed.substring(0, schemeEnd).toLowerCase();
    final websocket = scheme == 'ws' || scheme == 'wss';
    final nextScheme = websocket
        ? (secure ? 'wss' : 'ws')
        : (secure ? 'https' : 'http');
    return '$nextScheme://$currentAuthority$tail';
  }
}

String? _originOrNull(String url) {
  try {
    return normalizeRelayOrigin(url);
  } on FormatException {
    return null;
  } on ArgumentError {
    return null;
  }
}

/// Test seam: replaces this build's [ServiceAddresses]. Reset to `null` in
/// `tearDown`. App code never writes it.
@visibleForTesting
ServiceAddresses? debugServiceAddressesOverride;

final _buildServiceAddresses = ServiceAddresses.fromDefines(
  environment: const String.fromEnvironment('KUZZ_ENV'),
  relayUrl: const String.fromEnvironment('KUZZ_RELAY_URL'),
  keyBackupOrigin: const String.fromEnvironment('KUZZ_KEY_BACKUP_ORIGIN'),
  previousRelayUrls: const String.fromEnvironment('KUZZ_PREVIOUS_RELAY_URLS'),
);

/// This build's service addresses (see [ServiceAddresses.fromDefines]).
ServiceAddresses get appServiceAddresses =>
    debugServiceAddressesOverride ?? _buildServiceAddresses;

/// This build's storage scope (`kuzz` prod, `kuzz-dev` dev).
String get serviceScope => appServiceAddresses.scope;

/// See [ServiceAddresses.isServiceAddress].
bool isServiceAddress(String url) => appServiceAddresses.isServiceAddress(url);

/// See [ServiceAddresses.storageScopeFor].
String storageScopeFor(String url) => appServiceAddresses.storageScopeFor(url);

/// See [ServiceAddresses.currentServiceUrl].
String currentServiceUrl(String url) =>
    appServiceAddresses.currentServiceUrl(url);
