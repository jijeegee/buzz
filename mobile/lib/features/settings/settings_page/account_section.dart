part of '../settings_page.dart';

/// The signed-in account of the active community, as on desktop: who is
/// signed in, then Devices & security in a card of its own. A token account
/// also edits its global profile here. Account deletion sits folded at the
/// bottom of Settings.
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
    void openDevices() => unawaited(
      Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) => DevicesPage(origin: origin, agents: !googleKey),
        ),
      ),
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AppListCard(
          label: context.l10n.settingsAccountTitle,
          verticalPadding: Grid.twelve,
          children: [
            if (googleKey)
              AppListRow(
                key: const Key('settings-google-account-row'),
                icon: LucideIcons.user,
                // Name over email, like desktop; the email alone when the
                // account has no name, and a plain label before either loads.
                title: hasName
                    ? name
                    : email ?? context.l10n.settingsSignedInWithGoogle,
                subtitle: hasName
                    ? email ?? context.l10n.settingsSignedInWithGoogle
                    : email == null
                    ? null
                    : context.l10n.settingsSignedInWithGoogle,
              )
            else
              AppListRow(
                key: const Key('settings-account-row'),
                icon: LucideIcons.user,
                title: name == null || name.isEmpty
                    ? context.l10n.navProfile
                    : name,
                subtitle: username == null ? null : '@$username',
                trailing: const _RowChevron(),
                onTap: () => unawaited(_openAccountPage(context, origin)),
              ),
          ],
        ),
        AppListCard(
          label: context.l10n.settingsDevicesTitle,
          children: [
            AppListRow(
              key: const Key('settings-devices-row'),
              icon: LucideIcons.monitorSmartphone,
              title: context.l10n.settingsDevicesSignedIn,
              trailing: const _RowChevron(),
              onTap: openDevices,
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
