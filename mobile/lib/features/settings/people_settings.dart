import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/kuzz_links.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../groups/group_add_policy.dart';
import 'blocked_users_page.dart';
import '../people/people_errors.dart';
import 'kuzz_id_page.dart';

/// Settings → "Friends & privacy": whether people can find me by ID or
/// email, who can add me to groups, and the people I blocked.
class PeopleSettings extends HookConsumerWidget {
  const PeopleSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final origin = ref.watch(accountOriginProvider);
    if (origin == null) {
      // Without an account only the relay-side group setting applies.
      return AppListCard(
        key: const ValueKey('settings-people'),
        label: context.l10n.settingsPeopleSection,
        verticalPadding: Grid.twelve,
        children: const [GroupAddPolicyRow()],
      );
    }
    final profile = ref.watch(accountProfileProvider(origin));
    final loaded = profile.value;
    // The switch shows the requested value while the server saves it.
    final pending = useState<bool?>(null);
    final discoverable = pending.value ?? loaded?.discoverable ?? true;

    Future<void> setDiscoverable(bool value) async {
      final messenger = ScaffoldMessenger.maybeOf(context);
      final container = ProviderScope.containerOf(context, listen: false);
      final api = ref.read(accountApiProvider(origin));
      final failedMessage = context.l10n.settingsPeopleChangeFailed;
      pending.value = value;
      try {
        await api.setDiscoverable(value);
        container.invalidate(accountProfileProvider(origin));
        await container.read(accountProfileProvider(origin).future);
      } catch (error) {
        messenger?.showSnackBar(
          SnackBar(
            content: Text(friendlyError(error, fallback: failedMessage)),
          ),
        );
      } finally {
        if (context.mounted) pending.value = null;
      }
    }

    return AppListCard(
      key: const ValueKey('settings-people'),
      label: context.l10n.settingsPeopleSection,
      verticalPadding: Grid.twelve,
      children: [
        MergeSemantics(
          child: AppListRow(
            key: const Key('settings-discoverable-row'),
            icon: LucideIcons.search,
            title: context.l10n.settingsPeopleDiscoverable,
            subtitle: context.l10n.settingsPeopleDiscoverableSubtitle,
            trailing: Switch.adaptive(
              key: const Key('settings-discoverable-switch'),
              value: discoverable,
              onChanged: loaded == null || pending.value != null
                  ? null
                  : (value) => unawaited(setDiscoverable(value)),
            ),
          ),
        ),
        const GroupAddPolicyRow(),
        AppListRow(
          key: const Key('settings-blocked-row'),
          icon: LucideIcons.ban,
          title: context.l10n.settingsBlockedTitle,
          trailing: Icon(
            LucideIcons.chevronRightDir,
            size: 18,
            color: context.colors.onSurfaceVariant,
          ),
          onTap: () => unawaited(
            Navigator.of(context).push(
              MaterialPageRoute<void>(builder: (_) => const BlockedUsersPage()),
            ),
          ),
        ),
      ],
    );
  }
}

/// Profile → "Kuzz ID": the ID people use to find and add me. Hidden
/// without an account.
class KuzzIdSettings extends ConsumerWidget {
  const KuzzIdSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final origin = ref.watch(accountOriginProvider);
    if (origin == null) return const SizedBox.shrink();
    final loaded = ref.watch(accountProfileProvider(origin)).value;
    final username = loaded?.username;
    return AppListCard(
      key: const ValueKey('profile-kuzz-id'),
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-kuzz-id-row'),
          icon: LucideIcons.atSign,
          title: context.l10n.settingsKuzzIdTitle,
          value: username == null
              ? context.l10n.settingsKuzzIdNotSet
              : '@$username',
          trailing: Icon(
            LucideIcons.chevronRightDir,
            size: 18,
            color: context.colors.onSurfaceVariant,
          ),
          onTap: loaded == null
              ? null
              : () => unawaited(
                  Navigator.of(context).push(
                    MaterialPageRoute<void>(
                      builder: (_) => KuzzIdPage(origin: origin),
                    ),
                  ),
                ),
        ),
      ],
    );
  }
}

/// Settings → "Help": a way to reach the Kuzz team.
class SupportSettings extends StatelessWidget {
  const SupportSettings({super.key});

  @override
  Widget build(BuildContext context) {
    return AppListCard(
      key: const ValueKey('settings-support'),
      label: context.l10n.settingsHelpSection,
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-contact-row'),
          icon: LucideIcons.mail,
          title: context.l10n.settingsHelpContactUs,
          subtitle: kuzzContactEmail,
          onTap: () => unawaited(_contact(context)),
        ),
      ],
    );
  }

  Future<void> _contact(BuildContext context) async {
    final messenger = ScaffoldMessenger.maybeOf(context);
    final noEmailApp = context.l10n.settingsHelpNoEmailApp(kuzzContactEmail);
    var opened = false;
    try {
      opened = await launchUrl(kuzzContactUri);
    } catch (_) {
      opened = false;
    }
    if (!opened) {
      messenger?.showSnackBar(SnackBar(content: Text(noEmailApp)));
    }
  }
}
