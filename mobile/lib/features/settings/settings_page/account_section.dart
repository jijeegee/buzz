part of '../settings_page.dart';

/// The token account of the active community: its global profile, devices
/// and principal id. Replaces the nsec identity row in token mode.
class _AccountSection extends ConsumerWidget {
  const _AccountSection({required this.origin});

  final String origin;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(accountProfileProvider(origin)).value;
    final principal = ref.watch(activeCommunityProvider).value?.pubkey;
    final name = profile?.displayName;
    final username = profile?.username;
    return AppListCard(
      label: 'Account',
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-account-row'),
          icon: LucideIcons.user,
          title: name == null || name.isEmpty ? 'Profile' : name,
          subtitle: username == null ? null : '@$username',
          trailing: const _RowChevron(),
          onTap: () => unawaited(_openAccountPage(context, origin)),
        ),
        AppListRow(
          key: const Key('settings-devices-row'),
          icon: LucideIcons.monitorSmartphone,
          title: 'Devices & security',
          trailing: const _RowChevron(),
          onTap: () => unawaited(
            Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => DevicesPage(origin: origin),
              ),
            ),
          ),
        ),
        // The principal is the account's public key; the full npub is the
        // canonical copy form, and an unreadable one is never copied.
        if (principal != null && principal.isNotEmpty)
          _AccountIdRow(principal: principal),
      ],
    );
  }
}

Future<void> _openAccountPage(BuildContext context, String origin) =>
    Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => AccountPage(origin: origin)),
    );

/// One actionable node: the merged button owns both the label and the copy
/// action, so screen-reader activation copies like a tap (AGENTS.md rule 7).
class _AccountIdRow extends StatelessWidget {
  const _AccountIdRow({required this.principal});

  final String principal;

  @override
  Widget build(BuildContext context) {
    final npub = fullNpub(principal);
    final VoidCallback? copy = npub == null
        ? null
        : () => unawaited(
            copyToClipboard(context, npub, message: 'Account ID copied'),
          );
    return Semantics(
      container: true,
      button: true,
      enabled: copy != null,
      label: 'Copy account public key',
      value: npub ?? 'Identity unavailable',
      onTap: copy,
      excludeSemantics: true,
      child: AppListRow(
        key: const Key('settings-account-id-row'),
        icon: LucideIcons.key,
        title: 'Account ID',
        subtitle: shortPubkey(principal),
        trailing: Icon(
          LucideIcons.copy,
          size: 18,
          color: context.colors.onSurfaceVariant,
        ),
        onTap: copy,
      ),
    );
  }
}
