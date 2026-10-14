import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../goals/goal_provider.dart';
import 'channel.dart';
import 'channel_management_provider.dart';

/// Per-room goals switch. Saves immediately as a kind:9002 `goals` tag.
///
/// Shows the requested value while the save is in flight. A successful save
/// reports the new value through [onSaved] so the owner can update the
/// [channel] it passes in; a failed save reverts and shows a snackbar.
/// Read-only when [canEdit] is false.
class RoomGoalsSwitch extends HookConsumerWidget {
  const RoomGoalsSwitch({
    super.key,
    required this.channel,
    required this.canEdit,
    this.onSaved,
    this.contentPadding,
  });

  final Channel channel;
  final bool canEdit;
  final ValueChanged<bool>? onSaved;
  final EdgeInsetsGeometry? contentPadding;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pending = useState<bool?>(null);
    final value = pending.value ?? channel.goalsEnabled;
    final saving = pending.value != null;

    Future<void> save(bool next) async {
      pending.value = next;
      try {
        await ref
            .read(channelActionsProvider)
            .updateChannel(channelId: channel.id, goalsEnabled: next);
        // The relay hides the tree while goals are off; re-read it.
        ref.invalidate(goalTreeProvider(channel.id));
        if (context.mounted) onSaved?.call(next);
      } catch (_) {
        if (context.mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('Couldn’t update goals. Try again.')),
          );
        }
      } finally {
        if (context.mounted) pending.value = null;
      }
    }

    return SwitchListTile.adaptive(
      key: const ValueKey('room-goals-switch'),
      contentPadding: contentPadding,
      title: const Text('Goals'),
      subtitle: const Text(
        'Track goals for this conversation. Agents get goal guidance only '
        'when on.',
      ),
      value: value,
      onChanged: canEdit && !saving ? save : null,
    );
  }
}
