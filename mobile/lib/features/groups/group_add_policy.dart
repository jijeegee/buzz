import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import 'group_choice_sheet.dart';
import 'groups_api.dart';

/// Who can add me to groups: `friends` (default) or `everyone`.
final groupAddPolicyProvider = FutureProvider.autoDispose<String>(
  (ref) => ref.watch(groupsApiProvider).addPolicy(),
  retry: noGroupsRetry,
);

/// Settings → Friends & privacy → "Who can add me to groups".
class GroupAddPolicyRow extends HookConsumerWidget {
  const GroupAddPolicyRow({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final policyAsync = ref.watch(groupAddPolicyProvider);
    // Shows the picked value while the server saves it.
    final pending = useState<String?>(null);
    final policy = pending.value ?? policyAsync.value;

    Future<void> change() async {
      final current = policy ?? 'friends';
      final next = await showGroupChoiceSheet<String>(
        context: context,
        title: context.l10n.groupsAddPolicyTitle,
        selected: current,
        options: [
          GroupChoice(
            value: 'friends',
            label: context.l10n.groupsAddPolicyFriends,
            key: const ValueKey('group-add-policy-friends'),
          ),
          GroupChoice(
            value: 'everyone',
            label: context.l10n.groupsAddPolicyEveryone,
            key: const ValueKey('group-add-policy-everyone'),
          ),
        ],
      );
      if (next == null || next == current || !context.mounted) return;
      final messenger = ScaffoldMessenger.maybeOf(context);
      final l10n = context.l10n;
      pending.value = next;
      try {
        await ref.read(groupsApiProvider).setAddPolicy(next);
        ref.invalidate(groupAddPolicyProvider);
        await ref.read(groupAddPolicyProvider.future);
      } catch (_) {
        messenger?.showSnackBar(
          SnackBar(content: Text(l10n.groupsSettingChangeError)),
        );
      } finally {
        if (context.mounted) pending.value = null;
      }
    }

    return AppListRow(
      key: const Key('settings-group-add-policy-row'),
      icon: LucideIcons.userPlus,
      title: context.l10n.groupsAddPolicyTitle,
      value: switch (policy) {
        'everyone' => context.l10n.groupsAddPolicyEveryone,
        'friends' => context.l10n.groupsAddPolicyFriends,
        _ => null,
      },
      trailing: Icon(
        LucideIcons.chevronRight,
        size: 18,
        color: context.colors.onSurfaceVariant,
      ),
      onTap: (policy == null && policyAsync.isLoading) || pending.value != null
          ? null
          : () => unawaited(change()),
    );
  }
}
