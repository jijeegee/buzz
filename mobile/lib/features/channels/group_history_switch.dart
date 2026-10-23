import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import 'channel.dart';
import 'channel_management_provider.dart';

/// Whether people who join a group later see the messages sent before they
/// joined. Saves immediately as a kind:9002 `history` tag (`all` when on,
/// `since_join` when off).
///
/// Groups only: DMs and group chats always hide earlier messages. Shows the
/// requested value while the save is in flight; a successful save reports
/// the new value through [onSaved], a failed one reverts with a snackbar.
class GroupHistorySwitch extends HookConsumerWidget {
  const GroupHistorySwitch({
    super.key,
    required this.channel,
    this.onSaved,
    this.contentPadding,
  });

  final Channel channel;
  final ValueChanged<bool>? onSaved;
  final EdgeInsetsGeometry? contentPadding;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pending = useState<bool?>(null);
    final value = pending.value ?? channel.historyAll;
    final saving = pending.value != null;

    Future<void> save(bool next) async {
      pending.value = next;
      try {
        await ref
            .read(channelActionsProvider)
            .updateChannel(channelId: channel.id, historyAll: next);
        if (context.mounted) onSaved?.call(next);
      } catch (_) {
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text(context.l10n.groupHistorySaveError)),
          );
        }
      } finally {
        if (context.mounted) pending.value = null;
      }
    }

    return SwitchListTile.adaptive(
      key: const ValueKey('group-history-switch'),
      contentPadding: contentPadding,
      title: Text(context.l10n.groupHistorySwitchTitle),
      value: value,
      onChanged: saving ? null : save,
    );
  }
}
