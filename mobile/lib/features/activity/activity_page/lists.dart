part of '../activity_page.dart';

/// Reminders surface for the Reminders filter — due/pending NIP-ER
/// reminders that deep-link to their target message.
class _RemindersList extends ConsumerWidget {
  final ScrollController scrollController;
  final void Function(Reminder reminder) onOpen;
  final Future<void> Function() onRefresh;

  const _RemindersList({
    required this.scrollController,
    required this.onOpen,
    required this.onRefresh,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final remindersAsync = ref.watch(remindersProvider);
    final reminders = [
      for (final r in remindersAsync.value ?? const <Reminder>[])
        if (r.status == 'pending') r,
    ];

    if (remindersAsync.isLoading && reminders.isEmpty) {
      return const Center(
        child: KuzzLoadingIndicator(
          size: 44,
          semanticLabel: 'Loading reminders',
        ),
      );
    }
    if (reminders.isEmpty) {
      return const _EmptySurface(
        icon: LucideIcons.clock,
        message: 'No reminders',
        detail: 'Reminders you set will show up here.',
      );
    }

    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    return BeeRefreshIndicator(
      onRefresh: onRefresh,
      child: ListView.builder(
        controller: scrollController,
        padding: _activityScrollPadding(context),
        itemCount: reminders.length,
        itemBuilder: (context, index) {
          final reminder = reminders[index];
          final due = reminder.isDue(now);
          final title = reminder.note?.trim().isNotEmpty == true
              ? reminder.note!.trim()
              : reminder.target?.preview ?? 'Reminder';
          return ListTile(
            key: ValueKey('reminder-row-${reminder.id}'),
            leading: Icon(
              due ? LucideIcons.bellRing : LucideIcons.clock,
              size: 20,
              color: due
                  ? context.colors.primary
                  : context.colors.onSurfaceVariant,
            ),
            title: Text(title, maxLines: 2, overflow: TextOverflow.ellipsis),
            subtitle: reminder.notBefore != null
                ? Text(
                    due
                        ? 'Due now'
                        : 'Due ${_inboxTimestamp(reminder.notBefore!)}',
                  )
                : null,
            onTap: () => onOpen(reminder),
          );
        },
      ),
    );
  }
}

/// Drafts surface for the Drafts filter — locally saved unsent composer
/// text that reopens the target composer.
class _DraftsList extends ConsumerWidget {
  final List<ComposeDraft> drafts;
  final ScrollController scrollController;
  final Map<String, Channel> channelById;
  final String? myPubkey;
  final void Function(ComposeDraft draft) onOpen;
  final void Function(ComposeDraft draft) onDelete;

  const _DraftsList({
    required this.drafts,
    required this.scrollController,
    required this.channelById,
    required this.myPubkey,
    required this.onOpen,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final names = ref.watch(identityNameSourcesProvider);
    final privateNames = ref.watch(channelNamesProvider);
    if (drafts.isEmpty) {
      return const _EmptySurface(
        icon: LucideIcons.filePen,
        message: 'No drafts',
        detail: 'Unsent messages you start composing will show up here.',
      );
    }

    return ListView.builder(
      controller: scrollController,
      padding: _activityScrollPadding(context),
      itemCount: drafts.length,
      itemBuilder: (context, index) {
        final draft = drafts[index];
        final channel = channelById[draft.channelId];
        final destination = channel == null
            ? 'Unavailable group'
            : channelTitle(
                channel,
                currentPubkey: myPubkey,
                names: names,
                privateName: privateNames.nameFor(channel.id),
              );
        return ListTile(
          key: ValueKey('draft-row-${draft.key}'),
          leading: Icon(
            LucideIcons.filePen,
            size: 20,
            color: context.colors.onSurfaceVariant,
          ),
          title: Text(draft.text, maxLines: 2, overflow: TextOverflow.ellipsis),
          subtitle: Text(
            draft.threadHeadId != null ? 'Thread in $destination' : destination,
          ),
          trailing: IconButton(
            icon: const Icon(LucideIcons.trash2, size: 18),
            tooltip: 'Delete draft',
            onPressed: () => onDelete(draft),
          ),
          onTap: () => onOpen(draft),
        );
      },
    );
  }
}

/// Inbox list timestamp — desktop's `formatInboxTimestamp`:
/// time today, "Yesterday", short date this year, dated otherwise.
String _inboxTimestamp(int unixSeconds, {DateTime? now}) =>
    conversationTimestamp(unixSeconds, now: now);
