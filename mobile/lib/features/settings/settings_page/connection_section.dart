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
      label: context.l10n.settingsProtectAccountSection,
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-protect-account'),
          icon: LucideIcons.shield,
          title: context.l10n.settingsContinueWithGoogle,
          subtitle: context.l10n.settingsContinueWithGoogleSubtitle,
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
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(content: Text(context.l10n.settingsSignedInWithGoogle)),
  );
}

class _SignOutSection extends HookConsumerWidget {
  const _SignOutSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final error = useState<String?>(null);
    final hasBuiltInServer = ref.watch(hasBuiltInServerProvider);
    final (:accounts, :active) = _watchAccounts(ref);
    final name = active == null ? null : _knownAccountName(active);
    final others = [
      for (final account in accounts)
        if (account.id != active?.id) _accountName(context, account),
    ];
    Future<void> signOut() async {
      if (busy.value) return;
      final confirmed = await _confirmSignOut(
        context,
        hasBuiltInServer: hasBuiltInServer,
        name: name,
        others: others,
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
          error.value = context.l10n.settingsSignOutFailed;
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
          icon: LucideIcons.logOutDir,
          title: busy.value
              ? context.l10n.settingsSigningOut
              : name == null
              ? context.l10n.settingsSignOut
              : context.l10n.settingsSignOutOf(name),
          titleColor: name == null ? null : context.colors.error,
          subtitle: hasBuiltInServer
              ? null
              : context.l10n.settingsSignOutSubtitle,
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
          title: Text(context.l10n.settingsRemoveCommunity),
          children: [
            AppListRow(
              key: const Key('settings-remove-community'),
              icon: LucideIcons.trash2,
              title: context.l10n.settingsRemoveFromDevice,
              subtitle: context.l10n.settingsRemoveFromDeviceSubtitle,
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
      title: Text(context.l10n.settingsRemoveCommunityTitle),
      content: Text(
        usesPrivateKey
            ? '${context.l10n.settingsRemoveCommunityBody}\n\n'
                  '${context.l10n.settingsRemoveCommunityNoGoogleWarning}'
            : context.l10n.settingsRemoveCommunityBody,
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(),
          child: Text(context.l10n.commonCancel),
        ),
        FilledButton(
          onPressed: () async {
            final failedMessage = context.l10n.settingsRemoveCommunityFailed;
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
              ScaffoldMessenger.of(
                context,
              ).showSnackBar(SnackBar(content: Text(failedMessage)));
              return;
            }
            if (!context.mounted) return;
            // Pop all pushed routes back to root so MaterialApp.home rebuilds
            // to onboarding when auth state changes.
            Navigator.of(context).popUntil((route) => route.isFirst);
          },
          child: Text(context.l10n.settingsRemove),
        ),
      ],
    ),
  );
}

/// Sign-out ends the session of the account on screen, so it is confirmed
/// first, naming who stays signed in. Resolves to true only when the user
/// chose to sign out.
Future<bool?> _confirmSignOut(
  BuildContext context, {
  required bool hasBuiltInServer,
  required String? name,
  required List<String> others,
}) {
  final l10n = context.l10n;
  return showKuzzDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      key: const Key('settings-sign-out-dialog'),
      title: Text(
        name == null
            ? l10n.settingsSignOutTitle
            : l10n.settingsSignOutOfTitle(name),
      ),
      content: Text(switch (others) {
        [final other] => l10n.settingsSignOutOtherStays(other),
        [final other, ...] => l10n.settingsSignOutOthersStay(
          other,
          others.length - 1,
        ),
        _ =>
          hasBuiltInServer
              ? l10n.settingsSignOutBody
              : l10n.settingsSignOutBodyCommunities,
      }),
      actions: [
        TextButton(
          key: const Key('settings-sign-out-cancel'),
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: Text(context.l10n.commonCancel),
        ),
        FilledButton(
          key: const Key('settings-sign-out-confirm'),
          onPressed: () => Navigator.of(dialogContext).pop(true),
          child: Text(context.l10n.settingsSignOut),
        ),
      ],
    ),
  );
}
