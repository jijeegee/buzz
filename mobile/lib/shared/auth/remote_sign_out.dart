import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'account/account_api.dart';
import 'auth_provider.dart';
import 'token/session_key.dart';

/// Runs when the relay refuses one community's socket with `restricted:
/// signed_out` (signed out from another device). The socket's word alone
/// never signs anyone out: `/auth/me` on that community's own account must
/// answer 401 with a "session ended" code. Only then is that one community
/// signed out (device only, since the relay already ended the session); other
/// accounts stay signed in. A network failure, a live session or any other
/// answer leaves everything as it is.
///
/// Both are named by the caller, never read from the active community: it can
/// change under the socket that was refused. [isCurrent] holds while the
/// session asked about is still the account's: a sign-in finishing during the
/// check replaced it, and is not signed out by the old session's refusal.
typedef RemoteSignOutCheck =
    Future<void> Function({
      required String communityId,
      required SessionKey session,
      required bool Function() isCurrent,
    });

final remoteSignOutCheckProvider = Provider<RemoteSignOutCheck>((ref) {
  final running = <String, Future<void>>{};
  return ({required communityId, required session, required isCurrent}) =>
      running[communityId] ??=
          _check(
            ref,
            communityId: communityId,
            session: session,
            isCurrent: isCurrent,
          ).whenComplete(() {
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
  required SessionKey session,
  required bool Function() isCurrent,
}) async {
  try {
    await ref.read(accountApiProvider(session)).me();
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
      .signOutCommunity(communityId, deviceOnly: true, isCurrent: isCurrent);
}
