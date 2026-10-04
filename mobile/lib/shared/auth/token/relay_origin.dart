/// Canonical `scheme://host[:port]` origin for a relay URL.
///
/// Token sessions are scoped to this origin (plan §3.2): the refresh token
/// for one relay must never be presented to another. `ws`/`wss` fold to
/// `http`/`https`, the host is lower-cased, default ports and any path,
/// query or fragment are dropped. A bare host is treated as `https`.
///
/// Throws [FormatException] when [url] is not an http(s)/ws(s) URL with a
/// host.
String normalizeRelayOrigin(String url) {
  final trimmed = url.trim();
  final uri = Uri.parse(trimmed.contains('://') ? trimmed : 'https://$trimmed');
  final scheme = switch (uri.scheme.toLowerCase()) {
    'ws' => 'http',
    'wss' => 'https',
    final other => other,
  };
  if (scheme != 'http' && scheme != 'https') {
    throw FormatException('Unsupported relay URL scheme', url);
  }
  final host = uri.host.toLowerCase();
  if (host.isEmpty) {
    throw FormatException('Relay URL has no host', url);
  }
  final defaultPort = scheme == 'https' ? 443 : 80;
  final port = uri.hasPort && uri.port != defaultPort ? ':${uri.port}' : '';
  final printableHost = host.contains(':') ? '[$host]' : host;
  return '$scheme://$printableHost$port';
}
