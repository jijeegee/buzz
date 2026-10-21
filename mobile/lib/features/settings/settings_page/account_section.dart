part of '../settings_page.dart';

/// The signed-in account of the active community: who is signed in and its
/// security actions, then its devices in a card of their own. A token
/// account also edits its global profile here.
class _AccountSection extends ConsumerWidget {
  const _AccountSection({required this.origin, this.googleKey = false});

  final String origin;

  /// Signed in with Google holding the account key. Its profile is edited
  /// from the profile header, so the identity row is a label, not a button.
  final bool googleKey;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(accountProfileProvider(origin)).value;
    final name = profile?.displayName;
    final username = profile?.username;
    final email = profile?.email;
    final hasName = name != null && name.isNotEmpty;
    void openPage(DevicesPagePart part) => unawaited(
      Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) =>
              DevicesPage(origin: origin, botTokens: !googleKey, part: part),
        ),
      ),
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AppListCard(
          label: 'Account',
          verticalPadding: Grid.twelve,
          children: [
            if (googleKey)
              AppListRow(
                key: const Key('settings-google-account-row'),
                icon: LucideIcons.user,
                // Name over email, like desktop; the email alone when the
                // account has no name, and a plain label before either loads.
                title: hasName ? name : email ?? 'Signed in with Google',
                subtitle: hasName
                    ? email ?? 'Signed in with Google'
                    : email == null
                    ? null
                    : 'Signed in with Google',
              )
            else
              AppListRow(
                key: const Key('settings-account-row'),
                icon: LucideIcons.user,
                title: name == null || name.isEmpty ? 'Profile' : name,
                subtitle: username == null ? null : '@$username',
                trailing: const _RowChevron(),
                onTap: () => unawaited(_openAccountPage(context, origin)),
              ),
            AppListRow(
              key: const Key('settings-security-row'),
              icon: LucideIcons.shield,
              title: 'Security',
              trailing: const _RowChevron(),
              onTap: () => openPage(DevicesPagePart.account),
            ),
          ],
        ),
        AppListCard(
          label: 'Devices',
          children: [
            AppListRow(
              key: const Key('settings-devices-row'),
              icon: LucideIcons.monitorSmartphone,
              title: 'Signed-in devices',
              trailing: const _RowChevron(),
              onTap: () => openPage(DevicesPagePart.devices),
            ),
          ],
        ),
      ],
    );
  }
}

Future<void> _openAccountPage(BuildContext context, String origin) =>
    Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => AccountPage(origin: origin)),
    );
