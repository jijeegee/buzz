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
      children: [_IdentityRow(nsec: nsec)],
    );
  }
}

/// Destructive, so it gets a container of its own rather than sitting at the
/// bottom of the connection group.
class _RemoveCommunitySection extends ConsumerWidget {
  const _RemoveCommunitySection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return AppListCard(
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          icon: LucideIcons.logOut,
          title: 'Remove community',
          titleColor: context.colors.error,
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
  showBuzzDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Remove Community'),
      content: const Text(
        'This will disconnect this community and sign this device out. '
        'You can sign in to it again later.',
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
                SnackBar(content: Text('Could not remove community: $error')),
              );
              return;
            }
            if (!context.mounted) return;
            // Pop all pushed routes back to root so MaterialApp.home rebuilds
            // to onboarding when auth state changes.
            Navigator.of(context).popUntil((route) => route.isFirst);
          },
          style: FilledButton.styleFrom(backgroundColor: ctx.colors.error),
          child: const Text('Remove'),
        ),
      ],
    ),
  );
}
