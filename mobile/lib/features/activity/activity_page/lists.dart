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
      return Center(
        child: KuzzLoadingIndicator(
          size: 44,
          semanticLabel: context.l10n.activityLoadingReminders,
        ),
      );
    }
    if (reminders.isEmpty) {
      return _EmptySurface(
        icon: LucideIcons.clock,
        message: context.l10n.activityNoReminders,
        detail: context.l10n.activityNoRemindersDetail,
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
              : reminder.target?.preview ??
                    context.l10n.activityReminderFallback;
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
                        ? context.l10n.activityReminderDueNow
                        : context.l10n.activityReminderDue(
                            _inboxTimestamp(reminder.notBefore!),
                          ),
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
      return _EmptySurface(
        icon: LucideIcons.filePen,
        message: context.l10n.activityNoDrafts,
        detail: context.l10n.activityNoDraftsDetail,
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
            ? context.l10n.activityUnavailableGroup
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
            draft.threadHeadId != null
                ? context.l10n.activityDraftThreadIn(destination)
                : destination,
          ),
          trailing: IconButton(
            icon: const Icon(LucideIcons.trash2, size: 18),
            tooltip: context.l10n.activityDeleteDraft,
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
