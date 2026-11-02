import 'dart:convert';
import 'dart:math';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:http/http.dart' as http;

import 'android_push.dart' show androidPushChannel;
import 'push_lease_revocation_outbox.dart'
    show canonicalKuzzPushRelayHttpUrl, canonicalKuzzPushRelayOrigin;
import 'push_transcript.dart';

/// Gateway enrollment of the Android FCM endpoint (stage B A4.1; NIP-PL
/// profile `kuzz-android`). A Dart port of KuzzPushKit
/// `KuzzDevPushEnrollmentDriver` with Android Key Attestation:
///
/// - endpoint: lowercase hex of the UTF-8 FCM token (at most 512 bytes);
/// - a fresh Keystore EC P-256 key per enrollment, its attestation chain as
///   the `attestation`, `key_id` = base64 SHA-256 of the leaf public key;
/// - later requests are signed by that key (DER ECDSA over the transcript).
///
/// Same journal rules as iOS: the signed enrollment is saved before the
/// first network attempt, a token change (new endpoint hash) enrolls
/// afresh, the newest grant for the relay key is reused, the generation is
/// reserved before delegation, and only a 404 replay of an expired
/// challenge drops the journal.
const kuzzAndroidPushAppProfile = 'kuzz-android';

/// FCM tokens over this many UTF-8 bytes are logged and refused (N11).
const maxFcmTokenBytes = 512;

/// Lowercase hex of the UTF-8 [token], or null (logged) when it is empty,
/// not printable ASCII, or over [maxFcmTokenBytes].
String? fcmEndpoint(String token) {
  final bytes = utf8.encode(token);
  if (bytes.isEmpty || bytes.any((byte) => byte < 0x21 || byte > 0x7e)) {
    debugPrint('[AndroidPush] FCM token refused: not printable ASCII');
    return null;
  }
  if (bytes.length > maxFcmTokenBytes) {
    debugPrint(
      '[AndroidPush] FCM token refused: ${bytes.length} bytes '
      '(limit $maxFcmTokenBytes)',
    );
    return null;
  }
  return _hex(bytes);
}

/// The attestation challenge for an enrollment: SHA-256 of the enrollment
/// transcript with an empty `key_id`.
///
/// NIP-PL as written asks for SHA-256 of the transcript that contains the
/// key's own `key_id`, which a real Keystore cannot produce: the challenge
/// is fixed when the key is generated, before its public key exists. This
/// form needs the matching gateway change (see the PR); the request still
/// carries the real `key_id`, which the gateway checks against the leaf.
List<int> androidAttestationChallenge({
  required String gatewayOrigin,
  required String challengeId,
  required String challenge,
  required String endpoint,
  required int endpointEpoch,
  required int expiresAt,
}) => sha256
    .convert(
      KuzzPushTranscript.enroll(
        gatewayOrigin: gatewayOrigin,
        challengeId: challengeId,
        challenge: challenge,
        keyId: '',
        appProfile: kuzzAndroidPushAppProfile,
        endpoint: endpoint,
        endpointEpoch: endpointEpoch,
        expiresAt: expiresAt,
      ),
    )
    .bytes;

/// Keystore keys for gateway enrollment (`kuzz/android_push`).
abstract interface class AndroidEnrollmentKeys {
  /// A new attested key under [alias]: the chain (standard base64 of the
  /// concatenated DER certificates, leaf first) and the leaf public key DER.
  Future<({String chain, List<int> publicKey})> create(
    String alias,
    List<int> challenge,
  );

  /// DER ECDSA P-256 SHA-256 signature over [data].
  Future<List<int>> sign(String alias, List<int> data);
}

class ChannelAndroidEnrollmentKeys implements AndroidEnrollmentKeys {
  const ChannelAndroidEnrollmentKeys();

  @override
  Future<({String chain, List<int> publicKey})> create(
    String alias,
    List<int> challenge,
  ) async {
    final result = await androidPushChannel.invokeMapMethod<String, Object?>(
      'enrollmentKeyCreate',
      {'alias': alias, 'challenge': Uint8List.fromList(challenge)},
    );
    final chain = result?['chain'];
    final publicKey = result?['publicKey'];
    if (chain is! String || publicKey is! Uint8List) {
      throw StateError('Keystore returned no attested key');
    }
    return (chain: chain, publicKey: publicKey);
  }

  @override
  Future<List<int>> sign(String alias, List<int> data) async {
    final signature = await androidPushChannel.invokeMethod<Uint8List>(
      'enrollmentKeySign',
      {'alias': alias, 'data': Uint8List.fromList(data)},
    );
    if (signature == null) throw StateError('Keystore returned no signature');
    return signature;
  }
}

class GatewayStatusException implements Exception {
  final String route;
  final int status;
  GatewayStatusException(this.route, this.status);

  @override
  String toString() => 'Push gateway $route answered $status';
}

/// One stored grant (JSON in secure storage).
class AndroidPushGrantRecord {
  final String gatewayOrigin;
  final String relayOrigin;
  final String relayPubkey;
  final String? relayMetadataPubkey;
  final String installationHandle;
  final String keyAlias;
  final String installationId;
  final String endpointGrant;
  final String endpointHash;
  final int endpointEpoch;
  final int generation;
  final int expiresAt;

  const AndroidPushGrantRecord({
    required this.gatewayOrigin,
    required this.relayOrigin,
    required this.relayPubkey,
    required this.relayMetadataPubkey,
    required this.installationHandle,
    required this.keyAlias,
    required this.installationId,
    required this.endpointGrant,
    required this.endpointHash,
    required this.endpointEpoch,
    required this.generation,
    required this.expiresAt,
  });

  factory AndroidPushGrantRecord.fromJson(Map<String, dynamic> json) =>
      AndroidPushGrantRecord(
        gatewayOrigin: json['gatewayOrigin'] as String,
        relayOrigin: json['relayOrigin'] as String,
        relayPubkey: json['relayPubkey'] as String,
        relayMetadataPubkey: json['relayMetadataPubkey'] as String?,
        installationHandle: json['installationHandle'] as String,
        keyAlias: json['keyAlias'] as String,
        installationId: json['installationId'] as String,
        endpointGrant: json['endpointGrant'] as String,
        endpointHash: json['endpointHash'] as String,
        endpointEpoch: json['endpointEpoch'] as int,
        generation: json['generation'] as int,
        expiresAt: json['expiresAt'] as int,
      );

  Map<String, dynamic> toJson() => {
    'gatewayOrigin': gatewayOrigin,
    'relayOrigin': relayOrigin,
    'relayPubkey': relayPubkey,
    'relayMetadataPubkey': relayMetadataPubkey,
    'installationHandle': installationHandle,
    'keyAlias': keyAlias,
    'installationId': installationId,
    'endpointGrant': endpointGrant,
    'endpointHash': endpointHash,
    'endpointEpoch': endpointEpoch,
    'generation': generation,
    'expiresAt': expiresAt,
  };

  /// The shape `KuzzPushEndpointGrant.fromMap` reads.
  Map<String, Object?> toGrantMap() => {
    'relayOrigin': relayOrigin,
    'relayPubkey': relayPubkey,
    'installationId': installationId,
    'endpointGrant': endpointGrant,
    'endpointHash': endpointHash,
    'appProfile': kuzzAndroidPushAppProfile,
    'endpointEpoch': endpointEpoch,
    'generation': generation,
    'expiresAt': expiresAt,
  };
}

/// Grants and pending enrollments, one secure-storage entry.
class AndroidPushEnrollmentStore {
  static const _key = 'kuzz_android_push_enrollment_v1';
  final FlutterSecureStorage _storage;

  const AndroidPushEnrollmentStore([
    this._storage = const FlutterSecureStorage(),
  ]);

  Future<({List<AndroidPushGrantRecord> grants, Map<String, dynamic> pending})>
  load() async {
    final raw = await _storage.read(key: _key);
    if (raw == null) {
      return (grants: <AndroidPushGrantRecord>[], pending: <String, dynamic>{});
    }
    final json = jsonDecode(raw) as Map<String, dynamic>;
    return (
      grants: [
        for (final grant in json['grants'] as List? ?? const [])
          AndroidPushGrantRecord.fromJson(
            Map<String, dynamic>.from(grant as Map),
          ),
      ],
      pending: Map<String, dynamic>.from(json['pending'] as Map? ?? const {}),
    );
  }

  Future<void> _save(
    List<AndroidPushGrantRecord> grants,
    Map<String, dynamic> pending,
  ) => _storage.write(
    key: _key,
    value: jsonEncode({
      'grants': [for (final grant in grants) grant.toJson()],
      'pending': pending,
    }),
  );

  Future<void> saveGrant(AndroidPushGrantRecord record) async {
    final state = await load();
    final grants = [
      for (final grant in state.grants)
        if (grant.gatewayOrigin != record.gatewayOrigin ||
            grant.relayOrigin != record.relayOrigin)
          grant,
      record,
    ];
    await _save(grants, state.pending);
  }

  Future<Map<String, dynamic>?> pending(String slot) async =>
      (await load()).pending[slot] as Map<String, dynamic>?;

  Future<void> savePending(String slot, Map<String, dynamic> record) async {
    final state = await load();
    await _save(state.grants, {...state.pending, slot: record});
  }

  Future<void> removePending(String slot) async {
    final state = await load();
    await _save(state.grants, {...state.pending}..remove(slot));
  }
}

class AndroidPushEnrollment {
  static const endpointEpoch = 1;

  final String gatewayOrigin;
  final AndroidPushEnrollmentStore store;
  final AndroidEnrollmentKeys keys;
  final http.Client client;
  final DateTime Function() now;
  final int lifetimeSeconds;
  final String Function(int bytes) randomHex;

  AndroidPushEnrollment({
    required String gatewayUrl,
    required this.client,
    this.store = const AndroidPushEnrollmentStore(),
    this.keys = const ChannelAndroidEnrollmentKeys(),
    DateTime Function()? now,
    this.lifetimeSeconds = 2592000,
    String Function(int bytes)? randomHex,
  }) : gatewayOrigin = KuzzPushTranscript.canonicalGatewayOrigin(gatewayUrl),
       now = now ?? DateTime.now,
       randomHex = randomHex ?? _randomHex;

  int get _nowSeconds => now().millisecondsSinceEpoch ~/ 1000;

  Future<List<AndroidPushGrantRecord>> grants() async => [
    for (final grant in (await store.load()).grants)
      if (grant.gatewayOrigin == gatewayOrigin) grant,
  ];

  /// Enrolls [token] for [relayUrl]'s current push key and returns the
  /// saved grant.
  Future<AndroidPushGrantRecord> enroll(String token, String relayUrl) =>
      _enroll(token, relayUrl, 0);

  Future<AndroidPushGrantRecord> _enroll(
    String token,
    String relayUrl,
    int depth,
  ) async {
    if (depth > 3) throw StateError('Push enrollment did not settle');
    final endpoint = fcmEndpoint(token);
    if (endpoint == null) {
      throw const FormatException('FCM token refused');
    }
    final relayOrigin = canonicalKuzzPushRelayOrigin(relayUrl);
    final relayKeys = await _relayKeys(canonicalKuzzPushRelayHttpUrl(relayUrl));
    final relayPubkey = relayKeys.pushPubkey;
    final endpointHash = sha256.convert(utf8.encode(token)).toString();
    final nowSeconds = _nowSeconds;
    final slot = '$gatewayOrigin|$relayOrigin';
    final state = await store.load();
    final storedForOrigin = state.grants
        .where(
          (grant) =>
              grant.gatewayOrigin == gatewayOrigin &&
              grant.relayOrigin == relayOrigin,
        )
        .firstOrNull;
    final pending = state.pending[slot] as Map<String, dynamic>?;

    if (pending != null &&
        (pending['relayPubkey'] != relayPubkey ||
            pending['endpointHash'] != endpointHash ||
            (pending['expiresAt'] as int) <= nowSeconds)) {
      // Finish an unexpired request with its own journaled endpoint first.
      if ((pending['expiresAt'] as int) > nowSeconds) {
        await _resume(slot, pending, state.grants, null);
      } else {
        await store.removePending(slot);
      }
      return _enroll(token, relayUrl, depth + 1);
    }

    final shared =
        state.grants
            .where(
              (grant) =>
                  grant.gatewayOrigin == gatewayOrigin &&
                  grant.relayPubkey == relayPubkey &&
                  grant.endpointHash == endpointHash &&
                  grant.endpointEpoch == endpointEpoch &&
                  grant.expiresAt > nowSeconds + 300,
            )
            .toList()
          ..sort((a, b) => b.generation.compareTo(a.generation));
    if (pending == null && shared.isNotEmpty) {
      // Reuse the newest delegation without invalidating sibling relays.
      final reused = shared.first;
      final record = AndroidPushGrantRecord(
        gatewayOrigin: gatewayOrigin,
        relayOrigin: relayOrigin,
        relayPubkey: relayPubkey,
        relayMetadataPubkey: relayKeys.metadataPubkey,
        installationHandle: reused.installationHandle,
        keyAlias: reused.keyAlias,
        installationId: storedForOrigin?.installationId ?? randomHex(16),
        endpointGrant: reused.endpointGrant,
        endpointHash: endpointHash,
        endpointEpoch: reused.endpointEpoch,
        generation: reused.generation,
        expiresAt: reused.expiresAt,
      );
      await store.saveGrant(record);
      return record;
    }

    final installation = state.grants
        .where(
          (grant) =>
              grant.gatewayOrigin == gatewayOrigin &&
              grant.endpointHash == endpointHash &&
              grant.endpointEpoch == endpointEpoch &&
              grant.expiresAt > nowSeconds,
        )
        .firstOrNull;
    final renewed = nowSeconds + lifetimeSeconds;
    final Map<String, dynamic> journal;
    if (pending != null) {
      journal = pending;
    } else if (installation != null) {
      // An attested installation delegates to a new relay key or a higher
      // generation without enrolling the token again.
      journal = {
        'relayOrigin': relayOrigin,
        'relayPubkey': relayPubkey,
        'endpoint': endpoint,
        'endpointHash': endpointHash,
        'expiresAt': installation.expiresAt > nowSeconds + 300
            ? installation.expiresAt
            : renewed,
        'installationId': storedForOrigin?.installationId ?? randomHex(16),
        'installationHandle': installation.installationHandle,
        'keyAlias': installation.keyAlias,
        'delegationGeneration': 0,
      };
      await store.savePending(slot, journal);
    } else {
      final challenge = await _challenge();
      final alias = 'kuzz.push.enroll.${randomHex(16)}';
      final created = await keys.create(
        alias,
        androidAttestationChallenge(
          gatewayOrigin: gatewayOrigin,
          challengeId: challenge.id,
          challenge: challenge.value,
          endpoint: endpoint,
          endpointEpoch: endpointEpoch,
          expiresAt: renewed,
        ),
      );
      journal = {
        'relayOrigin': relayOrigin,
        'relayPubkey': relayPubkey,
        'endpoint': endpoint,
        'endpointHash': endpointHash,
        'expiresAt': renewed,
        'installationId': storedForOrigin?.installationId ?? randomHex(16),
        'challengeId': challenge.id,
        'challenge': challenge.value,
        'keyAlias': alias,
        'keyId': base64.encode(sha256.convert(created.publicKey).bytes),
        'attestation': created.chain,
        'delegationGeneration': 0,
      };
      // The exact signed request is durable before the first attempt.
      await store.savePending(slot, journal);
    }

    if (pending != null) {
      final record = await _resume(
        slot,
        journal,
        state.grants,
        relayKeys.metadataPubkey,
      );
      return record ?? _enroll(token, relayUrl, depth + 1);
    }
    return _complete(slot, journal, state.grants, relayKeys.metadataPubkey);
  }

  Future<AndroidPushGrantRecord?> _resume(
    String slot,
    Map<String, dynamic> journal,
    List<AndroidPushGrantRecord> grants,
    String? relayMetadataPubkey,
  ) async {
    try {
      return await _complete(slot, journal, grants, relayMetadataPubkey);
    } on GatewayStatusException catch (error) {
      // Exact replay found no installation and its challenge expired: only
      // this answer lets the journal go.
      if (error.route == 'v1/installations' &&
          error.status == 404 &&
          journal['installationHandle'] == null) {
        await store.removePending(slot);
        return null;
      }
      rethrow;
    }
  }

  Future<AndroidPushGrantRecord> _complete(
    String slot,
    Map<String, dynamic> journal,
    List<AndroidPushGrantRecord> grants,
    String? relayMetadataPubkey,
  ) async {
    var pending = Map<String, dynamic>.from(journal);
    final endpoint = pending['endpoint'] as String;
    final relayPubkey = pending['relayPubkey'] as String;
    final expiresAt = pending['expiresAt'] as int;
    final nowSeconds = _nowSeconds;
    var handle = pending['installationHandle'] as String?;
    if (handle == null) {
      final response = await _post('v1/installations', 201, {
        'v': 1,
        'challenge_id': pending['challengeId'],
        'challenge': pending['challenge'],
        'key_id': pending['keyId'],
        'attestation': pending['attestation'],
        'app_profile': kuzzAndroidPushAppProfile,
        'endpoint': endpoint,
        'endpoint_epoch': endpointEpoch,
        'expires_at': expiresAt,
      });
      final returned = response['installation_handle'];
      if (returned is! String ||
          !_uuidPattern.hasMatch(returned) ||
          response['endpoint_epoch'] != endpointEpoch ||
          response['expires_at'] != expiresAt) {
        throw const FormatException('Invalid v1/installations answer');
      }
      handle = returned;
      pending = {...pending, 'installationHandle': handle};
      await store.savePending(slot, pending);
    }

    final current = grants
        .where(
          (grant) =>
              grant.gatewayOrigin == gatewayOrigin &&
              grant.installationHandle == handle &&
              grant.relayPubkey == relayPubkey,
        )
        .map((grant) => grant.generation)
        .fold<int>(0, max);
    final base = max(current, pending['delegationGeneration'] as int? ?? 0);
    final generation = base + 1;
    pending = {...pending, 'delegationGeneration': generation};
    // Reserved first: a committed delegation whose save fails is retried
    // at a strictly higher generation.
    await store.savePending(slot, pending);

    final challenge = await _challenge();
    final alias = pending['keyAlias'] as String;
    final transcript = KuzzPushTranscript.delegate(
      gatewayOrigin: gatewayOrigin,
      challengeId: challenge.id,
      challenge: challenge.value,
      installationHandle: handle,
      endpointEpoch: endpointEpoch,
      generation: generation,
      relayPubkey: relayPubkey,
      notBefore: nowSeconds,
      expiresAt: expiresAt,
    );
    final assertion = base64.encode(await keys.sign(alias, transcript));
    final response = await _post('v1/delegations', 201, {
      'v': 1,
      'challenge_id': challenge.id,
      'challenge': challenge.value,
      'installation_handle': handle,
      'endpoint_epoch': endpointEpoch,
      'generation': generation,
      'relay_pubkey': relayPubkey,
      'not_before': nowSeconds,
      'expires_at': expiresAt,
      'assertion': assertion,
    });
    final endpointGrant = response['endpoint_grant'];
    if (endpointGrant is! String ||
        endpointGrant.isEmpty ||
        utf8.encode(endpointGrant).length > 4096) {
      throw const FormatException('Invalid v1/delegations answer');
    }
    final record = AndroidPushGrantRecord(
      gatewayOrigin: gatewayOrigin,
      relayOrigin: pending['relayOrigin'] as String,
      relayPubkey: relayPubkey,
      relayMetadataPubkey: relayMetadataPubkey,
      installationHandle: handle,
      keyAlias: alias,
      installationId: pending['installationId'] as String,
      endpointGrant: endpointGrant,
      endpointHash: pending['endpointHash'] as String,
      endpointEpoch: endpointEpoch,
      generation: generation,
      expiresAt: expiresAt,
    );
    await store.saveGrant(record);
    await store.removePending(slot);
    return record;
  }

  Future<({String id, String value})> _challenge() async {
    final response = await _post('v1/installations/challenges', 200, {'v': 1});
    final id = response['challenge_id'];
    final value = response['challenge'];
    final expires = response['expires_at'];
    if (id is! String ||
        !_uuidPattern.hasMatch(id) ||
        value is! String ||
        !_isChallenge(value) ||
        expires is! int ||
        expires <= _nowSeconds) {
      throw const FormatException('Invalid challenge answer');
    }
    return (id: id, value: value);
  }

  Future<Map<String, dynamic>> _post(
    String route,
    int expected,
    Map<String, Object?> body,
  ) async {
    final response = await client.post(
      Uri.parse('$gatewayOrigin/$route'),
      headers: const {'Content-Type': 'application/json'},
      body: jsonEncode(body),
    );
    if (response.statusCode != expected) {
      throw GatewayStatusException(route, response.statusCode);
    }
    final decoded = jsonDecode(response.body);
    if (decoded is! Map) throw FormatException('Invalid $route answer');
    return Map<String, dynamic>.from(decoded);
  }

  Future<({String pushPubkey, String? metadataPubkey})> _relayKeys(
    String relayHttpUrl,
  ) async {
    final response = await client.get(
      Uri.parse(relayHttpUrl),
      headers: const {'Accept': 'application/nostr+json'},
    );
    if (response.statusCode != 200) {
      throw GatewayStatusException('NIP-11', response.statusCode);
    }
    final document = jsonDecode(response.body);
    final push = document is Map ? document['push'] : null;
    final keyList = push is Map ? push['keys'] : null;
    final current = [
      for (final key in keyList is List ? keyList : const [])
        if (key is Map && key['current'] == true) key['pubkey'],
    ];
    if (current.length != 1 ||
        current.single is! String ||
        !_hex64.hasMatch(current.single as String)) {
      throw const FormatException('NIP-11 has no single current push key');
    }
    final self = (document as Map)['self'];
    return (
      pushPubkey: current.single as String,
      metadataPubkey: self is String && _hex64.hasMatch(self) ? self : null,
    );
  }
}

final _uuidPattern = RegExp(
  r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$',
);
final _hex64 = RegExp(r'^[0-9a-f]{64}$');

bool _isChallenge(String value) {
  if (!RegExp(r'^[A-Za-z0-9_-]{43}$').hasMatch(value)) return false;
  try {
    return base64Url.decode('$value=').length == 32;
  } catch (_) {
    return false;
  }
}

String _hex(List<int> bytes) =>
    bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();

String _randomHex(int bytes) {
  final random = Random.secure();
  return _hex(List.generate(bytes, (_) => random.nextInt(256)));
}
