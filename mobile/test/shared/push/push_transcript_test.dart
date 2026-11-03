import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/push/android_push_enrollment.dart';
import 'package:kuzz/shared/push/push_transcript.dart';

/// The gateway's own known-answer transcripts.
void main() {
  final fixture =
      jsonDecode(
            File(
              '../crates/kuzz-push-gateway/tests/vectors/'
              'app_attest_transcripts.json',
            ).readAsStringSync(),
          )
          as Map<String, dynamic>;
  final inputs = fixture['inputs'] as Map<String, dynamic>;
  Map<String, dynamic> vector(String name) => (fixture['vectors'] as List)
      .cast<Map<String, dynamic>>()
      .singleWhere((vector) => vector['name'] == name);

  void expectVector(List<int> bytes, String name) {
    final expected = vector(name);
    expect(utf8.decode(bytes), expected['transcript']);
    expect(sha256.convert(bytes).toString(), expected['sha256']);
  }

  test('enroll matches the gateway byte for byte', () {
    expectVector(
      KuzzPushTranscript.enroll(
        gatewayOrigin: 'https://push.example',
        challengeId: inputs['challenge_id'] as String,
        challenge: inputs['challenge'] as String,
        keyId: inputs['key_id'] as String,
        appProfile: inputs['app_profile'] as String,
        endpoint: inputs['endpoint'] as String,
        endpointEpoch: 1,
        expiresAt: inputs['expires_at'] as int,
      ),
      'enroll',
    );
  });

  test('the Android Keystore challenge matches the gateway', () {
    // The gateway hashes the enrollment transcript with key_id "" (the
    // challenge is fixed before the key exists); the request's real key_id
    // is not part of it.
    final expected = vector('android_enroll_challenge');
    final endpoint = inputs['android_endpoint'] as String;
    expect(
      utf8.decode(
        KuzzPushTranscript.enroll(
          gatewayOrigin: 'https://push.example',
          challengeId: inputs['challenge_id'] as String,
          challenge: inputs['challenge'] as String,
          keyId: '',
          appProfile: kuzzAndroidPushAppProfile,
          endpoint: endpoint,
          endpointEpoch: 1,
          expiresAt: inputs['expires_at'] as int,
        ),
      ),
      expected['transcript'],
    );
    expect(
      Digest(
        androidAttestationChallenge(
          gatewayOrigin: 'https://push.example',
          challengeId: inputs['challenge_id'] as String,
          challenge: inputs['challenge'] as String,
          endpoint: endpoint,
          endpointEpoch: 1,
          expiresAt: inputs['expires_at'] as int,
        ),
      ).toString(),
      expected['sha256'],
    );
  });

  test('delegate matches the gateway byte for byte', () {
    expectVector(
      KuzzPushTranscript.delegate(
        gatewayOrigin: 'https://push.example',
        challengeId: (inputs['challenge_id'] as String).toUpperCase(),
        challenge: inputs['challenge'] as String,
        installationHandle: inputs['installation_handle'] as String,
        endpointEpoch: 1,
        generation: 1,
        relayPubkey: inputs['relay_pubkey'] as String,
        notBefore: inputs['not_before'] as int,
        expiresAt: inputs['expires_at'] as int,
      ),
      'delegate',
    );
  });

  test('escaping follows serde and refuses non-ASCII', () {
    expect(
      KuzzPushTranscript.escape('a"\\\n\u0001', field: 'f'),
      r'a\"\\\n\u0001',
    );
    expect(
      () => KuzzPushTranscript.escape('é', field: 'f'),
      throwsFormatException,
    );
  });

  test('gateway origin keeps only scheme, host and port', () {
    expect(
      KuzzPushTranscript.canonicalGatewayOrigin('HTTPS://Push.Example:8443/'),
      'https://push.example:8443',
    );
    for (final bad in [
      'ftp://push.example',
      'https://push.example/v1',
      'https://u@push.example',
      'https://push.example?q=1',
    ]) {
      expect(
        () => KuzzPushTranscript.canonicalGatewayOrigin(bad),
        throwsFormatException,
        reason: bad,
      );
    }
  });
}
