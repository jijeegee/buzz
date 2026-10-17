part of '../settings_page.dart';

/// An older account whose key Google does not hold yet (made before Google
/// sign-in): the one place it is asked to continue with Google, worded like
/// desktop. Accounts signed in with Google use [_AccountSection].
class _ProtectAccountSection extends ConsumerWidget {
  const _ProtectAccountSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final config = ref.watch(relayConfigProvider);
    final nsec = config.nsec;
    // The signing key itself is never surfaced: users only see Google.
    if (config.tokenAuth || nsec == null || nsec.isEmpty) {
      return const SizedBox.shrink();
    }
    if (ref.watch(authProvider).value?.community?.googleBackupAccountId !=
        null) {
      return const SizedBox.shrink();
    }
    return AppListCard(
      label: 'Protect your account',
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-protect-account'),
          icon: LucideIcons.shield,
          title: 'Continue with Google',
          subtitle:
              'Sign in on your other devices, and get your account back '
              'if this phone is lost',
          trailing: const _RowChevron(),
          onTap: () => unawaited(_connectGoogle(context, config.baseUrl)),
        ),
      ],
    );
  }
}

Future<void> _connectGoogle(BuildContext context, String baseUrl) async {
  final connected = await Navigator.of(context).push<bool>(
    MaterialPageRoute<bool>(
      builder: (_) => TokenSignInPage(
        backupOrigin: normalizeRelayOrigin(baseUrl),
        connectGoogle: true,
      ),
    ),
  );
  if (connected != true || !context.mounted) return;
  ScaffoldMessenger.of(
    context,
  ).showSnackBar(const SnackBar(content: Text('Signed in with Google')));
}

class _SignOutSection extends HookConsumerWidget {
  const _SignOutSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final error = useState<String?>(null);
    final hasBuiltInServer = ref.watch(hasBuiltInServerProvider);
    Future<void> signOut() async {
      if (busy.value) return;
      final confirmed = await _confirmSignOut(
        context,
        hasBuiltInServer: hasBuiltInServer,
      );
      if (confirmed != true || !context.mounted || busy.value) return;
      busy.value = true;
      error.value = null;
      try {
        await ref.read(authProvider.notifier).signOut();
        if (context.mounted) {
          Navigator.of(context).popUntil((route) => route.isFirst);
        }
      } catch (_) {
        if (context.mounted) {
          error.value =
              'Couldn’t sign out. Check your connection and try again.';
        }
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    return AppListCard(
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-sign-out'),
          icon: LucideIcons.logOut,
          title: busy.value ? 'Signing out…' : 'Sign out',
          subtitle: hasBuiltInServer
              ? null
              : 'Your communities stay on this device',
          onTap: busy.value ? null : () => unawaited(signOut()),
        ),
        if (error.value != null)
          Semantics(liveRegion: true, child: Text(error.value!)),
      ],
    );
  }
}

/// Destructive removal is separate from ending a session.
class _RemoveCommunitySection extends ConsumerWidget {
  const _RemoveCommunitySection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return AppListCard(
      verticalPadding: Grid.twelve,
      children: [
        ExpansionTile(
          key: const Key('settings-removal-management'),
          title: const Text('Remove community'),
          children: [
            AppListRow(
              key: const Key('settings-remove-community'),
              icon: LucideIcons.trash2,
              title: 'Remove from this device',
              subtitle: 'Also removes its sign-in from this device',
              onTap: () => _confirmRemoveCommunity(context, ref),
            ),
          ],
        ),
      ],
    );
  }
}

void _confirmRemoveCommunity(BuildContext context, WidgetRef ref) {
  final usesPrivateKey = !ref.read(relayConfigProvider).tokenAuth;
  showKuzzDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Remove community from this device?'),
      content: Text(
        'This removes the community and its sign-in from this device. '
        'Your account and memberships on the server aren’t deleted.'
        '${usesPrivateKey ? '\n\nIf this community isn’t connected to a Google account, you won’t be able to sign back in to it.' : ''}',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () async {
            Navigator.of(ctx).pop(); // close dialog
            try {
              final removed = await removeCommunityWithRelaySignOut(
                context,
                ({required deviceOnly}) => ref
                    .read(authProvider.notifier)
                    .removeActiveCommunity(deviceOnly: deviceOnly),
              );
              if (!removed) return;
            } catch (error) {
              debugPrint('[Settings] remove community failed: $error');
              if (!context.mounted) return;
              ScaffoldMessenger.of(context).showSnackBar(
                const SnackBar(
                  content: Text('Couldn’t remove the community. Try again.'),
                ),
              );
              return;
            }
            if (!context.mounted) return;
            // Pop all pushed routes back to root so MaterialApp.home rebuilds
            // to onboarding when auth state changes.
            Navigator.of(context).popUntil((route) => route.isFirst);
          },
          child: const Text('Remove'),
        ),
      ],
    ),
  );
}

/// Sign-out ends every community's session on this device, so it is
/// confirmed first. Resolves to true only when the user chose to sign out.
Future<bool?> _confirmSignOut(
  BuildContext context, {
  required bool hasBuiltInServer,
}) {
  return showKuzzDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      key: const Key('settings-sign-out-dialog'),
      title: const Text('Sign out?'),
      content: Text(
        hasBuiltInServer
            ? 'You’ll be signed out on this device. Sign in with Google to '
                  'come back — your messages stay here.'
            : 'You’ll be signed out of all your communities on this device. '
                  'Sign in with Google to come back — your communities and '
                  'messages stay here.',
      ),
      actions: [
        TextButton(
          key: const Key('settings-sign-out-cancel'),
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const Key('settings-sign-out-confirm'),
          onPressed: () => Navigator.of(dialogContext).pop(true),
          child: const Text('Sign out'),
        ),
      ],
    ),
  );
}
