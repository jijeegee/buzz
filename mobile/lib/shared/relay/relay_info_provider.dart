import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import 'relay_provider.dart';

/// What the current relay advertises in its NIP-11 information document.
class RelayInfo {
  final Set<String> supportedExtensions;

  /// The relay's own signing pubkey (NIP-11 `self`), when advertised.
  final String? relaySelf;

  const RelayInfo({this.supportedExtensions = const {}, this.relaySelf});

  /// Parses the fields read here; anything else in the document is ignored.
  factory RelayInfo.fromDocument(Map<String, dynamic> document) {
    final extensions = document['supported_extensions'];
    final relaySelf = document['self'];
    return RelayInfo(
      supportedExtensions: extensions is List
          ? extensions.whereType<String>().toSet()
          : const {},
      relaySelf: relaySelf is String ? relaySelf : null,
    );
  }

  bool supports(String extension) => supportedExtensions.contains(extension);
}

typedef RelayInfoFetcher = Future<RelayInfo> Function(String relayBaseUrl);

/// Fetches a relay's NIP-11 document. Tests override this provider.
final relayInfoFetcherProvider = Provider<RelayInfoFetcher>(
  (ref) => fetchRelayInfo,
);

/// The current relay's NIP-11 information, or null when it could not be read.
///
/// Read on demand by features that depend on an optional relay extension, and
/// refetched when the relay changes. A failed read is not cached as "no
/// support" forever: callers invalidate this provider to retry on their next
/// user action.
final relayInfoProvider = FutureProvider<RelayInfo?>((ref) async {
  final baseUrl = ref.watch(relayConfigProvider.select((c) => c.baseUrl));
  try {
    return await ref.read(relayInfoFetcherProvider)(baseUrl);
  } catch (error) {
    debugPrint('[relayInfoProvider] NIP-11 unavailable for $baseUrl: $error');
    return null;
  }
});

Future<RelayInfo> fetchRelayInfo(
  String relayBaseUrl, {
  http.Client? client,
  Duration timeout = const Duration(seconds: 5),
}) async {
  final uri = Uri.tryParse(relayBaseUrl);
  if (uri == null ||
      !const {'http', 'https'}.contains(uri.scheme) ||
      uri.host.isEmpty) {
    throw FormatException('Invalid relay HTTP URL: $relayBaseUrl');
  }
  final ownedClient = client ?? http.Client();
  try {
    final response = await ownedClient
        .get(
          uri.resolve('/'),
          headers: const {'Accept': 'application/nostr+json'},
        )
        .timeout(timeout);
    if (response.statusCode != 200) {
      throw StateError(
        'NIP-11 request failed with HTTP ${response.statusCode}',
      );
    }
    final decoded = jsonDecode(response.body);
    if (decoded is! Map<String, dynamic>) {
      throw const FormatException('NIP-11 response must be a JSON object');
    }
    return RelayInfo.fromDocument(decoded);
  } finally {
    if (client == null) ownedClient.close();
  }
}
