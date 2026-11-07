part of '../settings_page.dart';

const double _accountHeadAvatarRadius = 36;
const double _accountRowAvatarRadius = 18;

/// The name an account goes by here. An account whose profile has not loaded
/// yet has none: its row's own name comes from the server address, which is
/// never shown.
String? _knownAccountName(Community account) {
  final name = account.displayName?.trim();
  return name != null && name.isNotEmpty ? name : account.email;
}

String _accountName(BuildContext context, Community account) =>
    _knownAccountName(account) ?? context.l10n.settingsAccountNoName;

/// The accounts signed in on this phone, and the one on screen among them.
({List<Community> accounts, Community? active}) _watchAccounts(WidgetRef ref) {
  final accounts = signedInAccountsByRecent(
    ref.watch(communityListProvider).value ?? const <Community>[],
    builtInOrigin: ref.watch(defaultCommunityOriginProvider),
  );
  final activeId = ref.watch(activeCommunityProvider).value?.id;
  return (
    accounts: accounts,
    active: accounts.where((row) => row.id == activeId).firstOrNull,
  );
}

class _AccountAvatar extends StatelessWidget {
  const _AccountAvatar({
    required this.account,
    required this.name,
    required this.radius,
  });

  final Community account;
  final String name;
  final double radius;

  @override
  Widget build(BuildContext context) {
    return AvatarImage(
      imageUrl: account.avatarUrl,
      radius: radius,
      backgroundColor: context.colors.primaryContainer,
      fallback: Text(
        name.characters.first.toUpperCase(),
        style: TextStyle(
          color: context.colors.onPrimaryContainer,
          fontSize: radius * 0.8,
          fontWeight: FontWeight.w600,
        ),
      ),
    );
  }
}

/// The account on screen, then every account signed in on this phone, most
/// recently used first, and **Add account**. A signed-out account is not
/// listed.
class _AccountsSection extends ConsumerWidget {
  const _AccountsSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final (:accounts, :active) = _watchAccounts(ref);
    if (active == null) return const SizedBox.shrink();
    final name = _accountName(context, active);
    return Column(
      key: const Key('settings-accounts'),
      children: [
        Padding(
          key: const Key('settings-account-head'),
          padding: const EdgeInsets.only(top: Grid.xs, bottom: Grid.xs),
          child: Column(
            children: [
              _AccountAvatar(
                account: active,
                name: name,
                radius: _accountHeadAvatarRadius,
              ),
              const SizedBox(height: Grid.xxs),
              Text(name, style: context.textTheme.titleLarge),
              if (active.email case final email? when email != name)
                Text(
                  email,
                  style: context.textTheme.bodyMedium?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
            ],
          ),
        ),
        AppListCard(
          label: context.l10n.settingsAccountsSection,
          children: [
            for (final account in accounts)
              _AccountRow(account: account, active: account.id == active.id),
            AppListRow(
              key: const Key('settings-add-account'),
              icon: LucideIcons.plus,
              title: context.l10n.settingsAddAccount,
              onTap: () => unawaited(
                Navigator.of(
                  context,
                  rootNavigator: true,
                ).pushNamed<void>(addCommunityRouteName),
              ),
            ),
          ],
        ),
      ],
    );
  }
}

class _AccountRow extends ConsumerWidget {
  const _AccountRow({required this.account, required this.active});

  final Community account;
  final bool active;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final name = _accountName(context, account);
    final email = account.email;
    return AppListRowRaw(
      key: ValueKey('settings-account-${account.id}'),
      leading: _AccountAvatar(
        account: account,
        name: name,
        radius: _accountRowAvatarRadius,
      ),
      title: Text(name, maxLines: 1, overflow: TextOverflow.ellipsis),
      subtitle: email == null || email == name
          ? null
          : Text(email, maxLines: 1, overflow: TextOverflow.ellipsis),
      trailing: active
          ? Icon(
              LucideIcons.check,
              key: const Key('settings-account-active'),
              size: 20,
              color: context.colors.primary,
            )
          : null,
      onTap: active
          ? null
          : () {
              unawaited(HapticFeedback.selectionClick());
              unawaited(
                ref
                    .read(communityListProvider.notifier)
                    .switchCommunity(account.id)
                    .catchError(
                      (Object error) =>
                          debugPrint('[Settings] switch failed: $error'),
                    ),
              );
            },
    );
  }
}
