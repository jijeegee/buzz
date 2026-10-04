import 'package:flutter/material.dart';

import '../auth/token/token_session.dart';
import '../theme/theme.dart';
import '../widgets/modal_presentation.dart';

/// One attempt to remove a community; [deviceOnly] skips the relay sign-out.
typedef CommunityRemovalAttempt =
    Future<void> Function({required bool deviceOnly});

enum _RelaySignOutChoice { retry, deviceOnly }

/// Runs [remove], signing a token community's device out on the relay first.
///
/// When the relay sign-out fails ([TokenSessionException]) nothing has been
/// removed, so the failure is shown and the user picks: try again, remove
/// from this device only (the device then stays listed on the account until
/// it is signed out elsewhere), or keep the community. Returns whether the
/// community was removed; any other error propagates to the caller.
Future<bool> removeCommunityWithRelaySignOut(
  BuildContext context,
  CommunityRemovalAttempt remove,
) async {
  var deviceOnly = false;
  while (true) {
    try {
      await remove(deviceOnly: deviceOnly);
      return true;
    } on TokenSessionException catch (error) {
      if (!context.mounted) return false;
      final choice = await showBuzzDialog<_RelaySignOutChoice>(
        context: context,
        builder: (dialogContext) =>
            _RelaySignOutFailedDialog(message: error.message),
      );
      switch (choice) {
        case _RelaySignOutChoice.retry:
          deviceOnly = false;
        case _RelaySignOutChoice.deviceOnly:
          deviceOnly = true;
        case null:
          return false;
      }
      if (!context.mounted) return false;
    }
  }
}

class _RelaySignOutFailedDialog extends StatelessWidget {
  const _RelaySignOutFailedDialog({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Couldn’t sign out on the relay'),
      content: Text(
        '$message\n\nRemoving it from this device only forgets the sign-in '
        'here. This device stays listed on your account until you sign it '
        'out from another device.',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Keep'),
        ),
        TextButton(
          key: const Key('remove-community-device-only'),
          onPressed: () =>
              Navigator.of(context).pop(_RelaySignOutChoice.deviceOnly),
          style: TextButton.styleFrom(foregroundColor: context.colors.error),
          child: const Text('Remove from this device only'),
        ),
        FilledButton(
          key: const Key('remove-community-retry'),
          onPressed: () => Navigator.of(context).pop(_RelaySignOutChoice.retry),
          child: const Text('Try again'),
        ),
      ],
    );
  }
}
