import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'blocked_users_provider.dart';
import 'friends_provider.dart';
import 'report.dart';

/// Blocking succeeded but the person is still in my friends list.
class FriendRemovalFailed implements Exception {
  const FriendRemovalFailed(this.cause);

  final Object cause;

  @override
  String toString() => 'Couldn’t remove them from your friends: $cause';
}

/// Blocks [pubkey], then removes them from my friends. Throws the server's
/// error when the block fails, or [FriendRemovalFailed] when only the friend
/// removal does (the block itself stands).
Future<void> blockPerson(WidgetRef ref, String pubkey) async {
  final normalized = pubkey.toLowerCase();
  await ref.read(blockedUsersProvider.notifier).block(normalized);
  final friends = ref.read(friendIdsProvider).value;
  if (friends != null && !friends.contains(normalized)) return;
  try {
    await ref.read(friendsControllerProvider).remove(normalized);
  } catch (error) {
    throw FriendRemovalFailed(error);
  }
}

/// Asks before blocking [name]. True when confirmed.
Future<bool> confirmBlock(BuildContext context, String name) async {
  final confirmed = await showDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: Text('Block $name?'),
      content: const Text(
        'They won’t be told. You won’t get their direct messages, their '
        'messages in shared channels are hidden, and they are removed from '
        'your friends. You can unblock them in Settings.',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: const Text('Cancel'),
        ),
        TextButton(
          key: const Key('confirm-block'),
          onPressed: () => Navigator.of(dialogContext).pop(true),
          style: TextButton.styleFrom(
            foregroundColor: dialogContext.colors.error,
          ),
          child: const Text('Block'),
        ),
      ],
    ),
  );
  return confirmed == true;
}

/// Confirms, blocks and reports the outcome in a snack bar. True when the
/// person is now blocked.
Future<bool> confirmAndBlock(
  BuildContext context,
  WidgetRef ref, {
  required String pubkey,
  required String name,
}) async {
  final messenger = ScaffoldMessenger.maybeOf(context);
  if (!await confirmBlock(context, name)) return false;
  try {
    await blockPerson(ref, pubkey);
  } on FriendRemovalFailed catch (error) {
    messenger?.showSnackBar(SnackBar(content: Text('Blocked $name. $error')));
    return true;
  } catch (error) {
    messenger?.showSnackBar(
      SnackBar(content: Text('Couldn’t block $name: $error')),
    );
    return false;
  }
  messenger?.showSnackBar(SnackBar(content: Text('Blocked $name')));
  return true;
}

/// Unblocks and reports the outcome in a snack bar.
Future<void> unblockWithFeedback(
  BuildContext context,
  WidgetRef ref, {
  required String pubkey,
  required String name,
}) async {
  final messenger = ScaffoldMessenger.maybeOf(context);
  try {
    await ref.read(blockedUsersProvider.notifier).unblock(pubkey);
  } catch (error) {
    messenger?.showSnackBar(
      SnackBar(content: Text('Couldn’t unblock $name: $error')),
    );
    return;
  }
  messenger?.showSnackBar(SnackBar(content: Text('Unblocked $name')));
}

/// Reports [pubkey] (a person) or, with [eventId], one of their messages:
/// pick a reason, optionally also block. True when the report was sent.
Future<bool> showReportSheet(
  BuildContext context, {
  required String pubkey,
  required String name,
  String? eventId,
}) async {
  final messenger = ScaffoldMessenger.maybeOf(context);
  final outcome = await showKuzzModalBottomSheet<String>(
    context: context,
    isScrollControlled: true,
    title: eventId == null ? 'Report $name' : 'Report message',
    builder: (_) => ReportSheet(pubkey: pubkey, name: name, eventId: eventId),
  );
  if (outcome == null) return false;
  messenger?.showSnackBar(SnackBar(content: Text(outcome)));
  return true;
}

/// The body of [showReportSheet]. Pops with the confirmation to show.
class ReportSheet extends HookConsumerWidget {
  const ReportSheet({
    super.key,
    required this.pubkey,
    required this.name,
    this.eventId,
  });

  final String pubkey;
  final String name;
  final String? eventId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final reason = useState<ReportReason?>(null);
    final alsoBlock = useState(true);
    final sending = useState(false);
    final error = useState<String?>(null);
    final alreadyBlocked = ref.watch(
      blockedPubkeysProvider.select((set) => set.contains(pubkey)),
    );
    // Blocking lives on the account; an older key-only community has none.
    final canBlock =
        !alreadyBlocked && ref.watch(accountOriginProvider) != null;

    Future<void> send() async {
      final chosen = reason.value;
      if (chosen == null || sending.value) return;
      sending.value = true;
      error.value = null;
      try {
        await ref
            .read(reportServiceProvider)
            .submit(pubkey: pubkey, reason: chosen, eventId: eventId);
      } catch (failure) {
        if (!context.mounted) return;
        sending.value = false;
        error.value = 'Couldn’t send the report: $failure';
        return;
      }
      var outcome = 'Report sent to the moderators';
      if (alsoBlock.value && canBlock) {
        try {
          await blockPerson(ref, pubkey);
          outcome = 'Report sent. $name is blocked.';
        } on FriendRemovalFailed catch (failure) {
          outcome = 'Report sent. $name is blocked. $failure';
        } catch (failure) {
          outcome = 'Report sent, but couldn’t block $name: $failure';
        }
      }
      if (context.mounted) Navigator.of(context).pop(outcome);
    }

    return SafeArea(
      top: false,
      child: SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(
          Grid.gutter,
          0,
          Grid.gutter,
          Grid.xs,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              'Reports go to this community’s moderators. $name isn’t told '
              'who reported them.',
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: Grid.xxs),
            RadioGroup<ReportReason>(
              groupValue: reason.value,
              onChanged: (value) {
                if (!sending.value) reason.value = value;
              },
              child: Column(
                children: [
                  for (final item in ReportReason.values)
                    RadioListTile<ReportReason>(
                      key: Key('report-reason-${item.wireValue}'),
                      value: item,
                      contentPadding: EdgeInsets.zero,
                      title: Text(item.label),
                    ),
                ],
              ),
            ),
            if (canBlock)
              CheckboxListTile(
                key: const Key('report-also-block'),
                value: alsoBlock.value,
                onChanged: sending.value
                    ? null
                    : (value) => alsoBlock.value = value ?? false,
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                title: Text('Also block $name'),
              ),
            if (error.value case final message?) ...[
              const SizedBox(height: Grid.xxs),
              Semantics(
                liveRegion: true,
                child: Text(
                  message,
                  key: const Key('report-error'),
                  style: context.textTheme.bodySmall?.copyWith(
                    color: context.colors.error,
                  ),
                ),
              ),
            ],
            const SizedBox(height: Grid.xs),
            FilledButton(
              key: const Key('report-submit'),
              onPressed: reason.value == null || sending.value
                  ? null
                  : () => unawaited(send()),
              child: sending.value
                  ? const KuzzLoadingIndicator(
                      size: 20,
                      semanticLabel: 'Sending report',
                    )
                  : const Text('Report'),
            ),
          ],
        ),
      ),
    );
  }
}
