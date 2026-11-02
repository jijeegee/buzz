import 'dart:convert';

/// The exact NIP-PL gateway transcripts (`docs/nips/NIP-PL.md`, "Exact App
/// Attest transcript construction"): `domain + "\n" + compact JSON` with
/// members in this order, serde_json string escaping and ASCII only. Same
/// bytes as Swift `KuzzPushTranscript` and the gateway's encoder
/// (crates/kuzz-push-gateway/tests/vectors/app_attest_transcripts.json).
abstract final class KuzzPushTranscript {
  static const enrollDomain = 'kuzz.push.enroll.v1';
  static const delegateDomain = 'kuzz.push.delegate.v1';
  static const wireVersion = 1;
  static const _audienceOrigin = 'https://push.buzz.xyz';

  static List<int> enroll({
    required String gatewayOrigin,
    required String challengeId,
    required String challenge,
    required String keyId,
    required String appProfile,
    required String endpoint,
    required int endpointEpoch,
    required int expiresAt,
  }) => _encode(enrollDomain, [
    _int('v', wireVersion),
    _string('audience', _audience(gatewayOrigin, 'v1/installations')),
    _uuid('challenge_id', challengeId),
    _string('challenge', challenge),
    _string('key_id', keyId),
    _string('app_profile', appProfile),
    _string('endpoint', endpoint),
    _int('endpoint_epoch', endpointEpoch),
    _int('expires_at', expiresAt),
  ]);

  static List<int> delegate({
    required String gatewayOrigin,
    required String challengeId,
    required String challenge,
    required String installationHandle,
    required int endpointEpoch,
    required int generation,
    required String relayPubkey,
    required int notBefore,
    required int expiresAt,
  }) => _encode(delegateDomain, [
    _int('v', wireVersion),
    _string('audience', _audience(gatewayOrigin, 'v1/delegations')),
    _uuid('challenge_id', challengeId),
    _string('challenge', challenge),
    _uuid('installation_handle', installationHandle),
    _int('endpoint_epoch', endpointEpoch),
    _int('generation', generation),
    _string('relay_pubkey', relayPubkey),
    _int('not_before', notBefore),
    _int('expires_at', expiresAt),
  ]);

  /// `scheme://host[:port]` of a configured gateway URL, or a
  /// [FormatException] for anything with a path, query or credentials.
  static String canonicalGatewayOrigin(String value) {
    final uri = Uri.tryParse(value);
    final scheme = uri?.scheme.toLowerCase();
    if (uri == null ||
        (scheme != 'http' && scheme != 'https') ||
        uri.host.isEmpty ||
        (uri.path.isNotEmpty && uri.path != '/') ||
        uri.userInfo.isNotEmpty ||
        uri.hasQuery ||
        uri.hasFragment) {
      throw const FormatException('Invalid push gateway URL');
    }
    final port = uri.hasPort ? ':${uri.port}' : '';
    return '$scheme://${uri.host.toLowerCase()}$port';
  }

  static String _audience(String gatewayOrigin, String route) {
    canonicalGatewayOrigin(gatewayOrigin);
    return '$_audienceOrigin/$route';
  }

  static List<int> _encode(String domain, List<String> members) =>
      utf8.encode('$domain\n{${members.join(',')}}');

  static String _int(String key, int value) => '"$key":$value';

  static String _uuid(String key, String value) {
    final lower = value.toLowerCase();
    if (!RegExp(
      r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$',
    ).hasMatch(lower)) {
      throw FormatException('Invalid UUID for $key');
    }
    return '"$key":"$lower"';
  }

  static String _string(String key, String value) =>
      '"$key":"${escape(value, field: key)}"';

  /// serde_json string escaping; non-ASCII is refused.
  static String escape(String value, {required String field}) {
    final out = StringBuffer();
    for (final unit in value.codeUnits) {
      switch (unit) {
        case 0x22:
          out.write(r'\"');
        case 0x5c:
          out.write(r'\\');
        case 0x08:
          out.write(r'\b');
        case 0x09:
          out.write(r'\t');
        case 0x0a:
          out.write(r'\n');
        case 0x0c:
          out.write(r'\f');
        case 0x0d:
          out.write(r'\r');
        case < 0x20:
          out.write('\\u${unit.toRadixString(16).padLeft(4, '0')}');
        case <= 0x7e:
          out.writeCharCode(unit);
        default:
          throw FormatException('Non-ASCII input in $field');
      }
    }
    return out.toString();
  }
}
