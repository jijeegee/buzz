import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/kuzz_links.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import 'blocked_users_page.dart';
import 'kuzz_id_page.dart';

/// Settings → "Friends & privacy": my Kuzz ID, whether people can find me
/// by ID or email, and the people I blocked. Hidden without an account.
class PeopleSettings extends HookConsumerWidget {
  const PeopleSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final origin = ref.watch(accountOriginProvider);
    if (origin == null) return const SizedBox.shrink();
    final profile = ref.watch(accountProfileProvider(origin));
    final loaded = profile.value;
    // The switch shows the requested value while the server saves it.
    final pending = useState<bool?>(null);
    final discoverable = pending.value ?? loaded?.discoverable ?? true;
    final username = loaded?.username;

    Future<void> setDiscoverable(bool value) async {
      final messenger = ScaffoldMessenger.maybeOf(context);
      pending.value = value;
      try {
        await ref.read(accountApiProvider(origin)).setDiscoverable(value);
        ref.invalidate(accountProfileProvider(origin));
        await ref.read(accountProfileProvider(origin).future);
      } catch (error) {
        messenger?.showSnackBar(
          SnackBar(content: Text('Couldn’t change this setting: $error')),
        );
      } finally {
        if (context.mounted) pending.value = null;
      }
    }

    return AppListCard(
      key: const ValueKey('settings-people'),
      label: 'Friends & privacy',
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-kuzz-id-row'),
          icon: LucideIcons.atSign,
          title: 'Kuzz ID',
          value: username == null ? 'Not set' : '@$username',
          trailing: Icon(
            LucideIcons.chevronRight,
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
        MergeSemantics(
          child: AppListRow(
            key: const Key('settings-discoverable-row'),
            icon: LucideIcons.search,
            title: 'Let people find me by ID or email',
            subtitle: 'Only someone who types your exact ID or email',
            trailing: Switch.adaptive(
              key: const Key('settings-discoverable-switch'),
              value: discoverable,
              onChanged: loaded == null || pending.value != null
                  ? null
                  : (value) => unawaited(setDiscoverable(value)),
            ),
          ),
        ),
        AppListRow(
          key: const Key('settings-blocked-row'),
          icon: LucideIcons.ban,
          title: 'Blocked users',
          trailing: Icon(
            LucideIcons.chevronRight,
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

/// Settings → "Help": a way to reach the Kuzz team.
class SupportSettings extends StatelessWidget {
  const SupportSettings({super.key});

  @override
  Widget build(BuildContext context) {
    return AppListCard(
      key: const ValueKey('settings-support'),
      label: 'Help',
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-contact-row'),
          icon: LucideIcons.mail,
          title: 'Contact us',
          subtitle: kuzzContactEmail,
          onTap: () => unawaited(_contact(context)),
        ),
      ],
    );
  }

  Future<void> _contact(BuildContext context) async {
    final messenger = ScaffoldMessenger.maybeOf(context);
    var opened = false;
    try {
      opened = await launchUrl(kuzzContactUri);
    } catch (_) {
      opened = false;
    }
    if (!opened) {
      messenger?.showSnackBar(
        const SnackBar(
          content: Text('No email app found. Write to $kuzzContactEmail'),
        ),
      );
    }
  }
}
