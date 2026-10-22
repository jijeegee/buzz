import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'account/account_api.dart';
import 'auth_provider.dart';

/// Runs when the relay refuses one community's socket with `restricted:
/// signed_out` (signed out from another device). The socket's word alone
/// never signs anyone out: `/auth/me` on that community's account origin
/// must answer 401 with a "session ended" code. Only then is that one
/// community signed out (device only, since the relay already ended the
/// session); other accounts stay signed in. A network failure, a live
/// session or any other answer leaves everything as it is.
typedef RemoteSignOutCheck =
    Future<void> Function({
      required String communityId,
      required String origin,
    });

final remoteSignOutCheckProvider = Provider<RemoteSignOutCheck>((ref) {
  final running = <String, Future<void>>{};
  return ({required communityId, required origin}) => running[communityId] ??=
      _check(ref, communityId: communityId, origin: origin).whenComplete(() {
        // A block body: returning the removed future would wait on itself.
        running.remove(communityId);
      });
});

/// Relay codes meaning the account session no longer exists.
const sessionEndedCodes = {
  'token_revoked',
  'principal_disabled',
  'invalid_token',
};

Future<void> _check(
  Ref ref, {
  required String communityId,
  required String origin,
}) async {
  try {
    await ref.read(accountApiProvider(origin)).me();
    return; // Still signed in: the refusal was about something else.
  } on AccountApiException catch (error) {
    final ended =
        error.statusCode == 401 && sessionEndedCodes.contains(error.code);
    if (!ended) return;
  } catch (error) {
    debugPrint('[RemoteSignOut] check failed: ${error.runtimeType}');
    return;
  }
  await ref
      .read(authProvider.notifier)
      .signOutCommunity(communityId, deviceOnly: true);
}
