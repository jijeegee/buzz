part of '../settings_page.dart';

class _ConnectionSection extends ConsumerWidget {
  const _ConnectionSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final config = ref.watch(relayConfigProvider);
    final nsec = config.nsec;
    // A token community's account lives on the relay (see _AccountSection);
    // never expose a leftover local key for it.
    if (config.tokenAuth || nsec == null || nsec.isEmpty) {
      return const SizedBox.shrink();
    }
    return AppListCard(
      label: 'Connection',
      verticalPadding: Grid.twelve,
      children: [
        _IdentityRow(nsec: nsec),
        AppListRow(
          key: const Key('settings-google-key-backup'),
          icon: LucideIcons.shield,
          title: 'Google key backup',
          subtitle: 'Link or recover this signing identity',
          trailing: const _RowChevron(),
          onTap: () => unawaited(
            Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => TokenSignInPage(
                  backupOrigin: normalizeRelayOrigin(config.baseUrl),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// Signs this device out of the current community, without deleting the account.
class _RemoveCommunitySection extends ConsumerWidget {
  const _RemoveCommunitySection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return AppListCard(
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          icon: LucideIcons.logOut,
          title: 'Sign out',
          subtitle: 'This community on this device',
          onTap: () => _confirmRemoveCommunity(context, ref),
        ),
      ],
    );
  }
}

class _IdentityRow extends StatelessWidget {
  const _IdentityRow({required this.nsec});

  final String nsec;

  @override
  Widget build(BuildContext context) {
    final privHex = nostr.Nip19.decode(payload: nsec).data;
    final npub = privHex.isNotEmpty
        ? fullNpub(nostr.Keys(privHex).public)
        : null;

    // The full npub is the canonical copy/share form (never raw hex); an
    // invalid identity is surfaced as unavailable and never copied.
    return Semantics(
      button: true,
      label: 'Copy identity public key',
      value: npub ?? 'Identity unavailable',
      child: AppListRow(
        icon: LucideIcons.key,
        title: 'Identity (pubkey)',
        trailing: Icon(
          LucideIcons.copy,
          size: 18,
          color: context.colors.onSurfaceVariant,
        ),
        onTap: npub == null
            ? null
            : () async {
                await copyToClipboard(context, npub, message: 'Pubkey copied');
              },
      ),
    );
  }
}

void _confirmRemoveCommunity(BuildContext context, WidgetRef ref) {
  final usesPrivateKey = !ref.read(relayConfigProvider).tokenAuth;
  showBuzzDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Sign out of this community?'),
      content: Text(
        'This signs this device out and removes the community from its list. '
        'Your account and community membership are not deleted. '
        'To return, sign in again or use your existing key or backup.'
        '${usesPrivateKey ? '\n\nThe saved private key for this community is removed from this device. Before continuing, make sure you have a saved key or backup, or have verified Google key recovery.' : ''}',
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
                    .signOut(deviceOnly: deviceOnly),
              );
              if (!removed) return;
            } catch (error) {
              if (!context.mounted) return;
              ScaffoldMessenger.of(context).showSnackBar(
                SnackBar(content: Text('Could not sign out: $error')),
              );
              return;
            }
            if (!context.mounted) return;
            // Pop all pushed routes back to root so MaterialApp.home rebuilds
            // to onboarding when auth state changes.
            Navigator.of(context).popUntil((route) => route.isFirst);
          },
          child: const Text('Sign out'),
        ),
      ],
    ),
  );
}
