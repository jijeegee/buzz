part of '../invite_create_page.dart';

class _InviteLinkSection extends HookConsumerWidget {
  const _InviteLinkSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final ttlSeconds = useState(defaultCommunityInviteTtlSeconds);
    final maxUses = useState<int?>(null);
    final generationId = useRef(0);
    final invite = useState<MintedCommunityInvite?>(null);
    final isGenerating = useState(true);
    final generationError = useState<String?>(null);

    Future<void> generate() async {
      final requestId = ++generationId.value;
      isGenerating.value = true;
      invite.value = null;
      generationError.value = null;
      try {
        final created = await ref
            .read(communityInviteActionsProvider)
            .mintInvite(ttlSeconds: ttlSeconds.value, maxUses: maxUses.value);
        if (context.mounted && requestId == generationId.value) {
          invite.value = created;
        }
      } catch (error) {
        if (context.mounted && requestId == generationId.value) {
          generationError.value = _inviteErrorMessage(error);
        }
      } finally {
        if (context.mounted && requestId == generationId.value) {
          isGenerating.value = false;
        }
      }
    }

    useEffect(() {
      unawaited(generate());
      return null;
    }, [ttlSeconds.value, maxUses.value]);

    final l10n = context.l10n;
    final ttlLabel = _ttlLabel(l10n, ttlSeconds.value);
    final maxUsesLabel = _maxUsesLabel(l10n, maxUses.value);
    final shareFailedMessage = l10n.inviteShareFailed;
    Future<void> share(BuildContext buttonContext) async {
      final url = invite.value?.url;
      if (url == null) return;
      final renderBox = buttonContext.findRenderObject() as RenderBox?;
      final origin = renderBox == null
          ? null
          : renderBox.localToGlobal(Offset.zero) & renderBox.size;
      try {
        await ref.read(shareCommunityInviteProvider)(url, origin);
      } catch (_) {
        if (!context.mounted) return;
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(shareFailedMessage)));
      }
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
          child: Row(
            children: [
              Expanded(
                child: Builder(
                  builder: (buttonContext) => KuzzActionTile(
                    key: const Key('community-invite-share-link'),
                    icon: LucideIcons.share2,
                    label: l10n.inviteShare,
                    isEnabled: invite.value != null,
                    onTap: () => share(buttonContext),
                  ),
                ),
              ),
              const SizedBox(width: Grid.twelve),
              Expanded(
                child: KuzzActionTile(
                  key: const Key('community-invite-copy-link'),
                  icon: LucideIcons.copy,
                  label: l10n.inviteCopy,
                  isEnabled: invite.value != null,
                  onTap: () => copyToClipboard(
                    context,
                    invite.value!.url,
                    message: l10n.inviteLinkCopied,
                  ),
                ),
              ),
            ],
          ),
        ),
        if (generationError.value case final error?) ...[
          const SizedBox(height: Grid.xs),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    error,
                    style: context.textTheme.bodySmall?.copyWith(
                      color: context.colors.error,
                    ),
                  ),
                ),
                TextButton(onPressed: generate, child: Text(l10n.inviteRetry)),
              ],
            ),
          ),
        ],
        const SizedBox(height: Grid.xs),
        AppListCard(
          key: const Key('community-invite-link-settings'),
          dividerIndent: Grid.xs,
          children: [
            AppListRow(
              key: const Key('community-invite-expiry-setting'),
              title: l10n.inviteExpiresAfter,
              value: ttlLabel,
              trailing: const _InviteRowChevron(),
              onTap: isGenerating.value
                  ? null
                  : () => _showInviteOptionSheet<int>(
                      context: context,
                      title: l10n.inviteExpiresAfter,
                      value: ttlSeconds.value,
                      options: communityInviteTtlOptions,
                      labelOf: (value) => _ttlLabel(l10n, value),
                      onSelected: (value) => ttlSeconds.value = value,
                    ),
            ),
            AppListRow(
              key: const Key('community-invite-max-uses-setting'),
              title: l10n.inviteLimitUses,
              value: maxUsesLabel,
              trailing: const _InviteRowChevron(),
              onTap: isGenerating.value
                  ? null
                  : () => _showInviteOptionSheet<int?>(
                      context: context,
                      title: l10n.inviteLimitUses,
                      value: maxUses.value,
                      options: communityInviteMaxUseOptions,
                      labelOf: (value) => _maxUsesLabel(l10n, value),
                      onSelected: (value) => maxUses.value = value,
                    ),
            ),
          ],
        ),
      ],
    );
  }
}

void _showInviteOptionSheet<T>({
  required BuildContext context,
  required String title,
  required T value,
  required List<CommunityInviteOption<T>> options,
  required String Function(T value) labelOf,
  required ValueChanged<T> onSelected,
}) {
  showKuzzModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    builder: (_) => _InviteOptionSheet<T>(
      title: title,
      value: value,
      options: options,
      labelOf: labelOf,
      onSelected: onSelected,
    ),
  );
}

/// Shown name of an invite-link lifetime in whole days.
String _ttlLabel(AppLocalizations l10n, int seconds) =>
    l10n.inviteExpiryDays(seconds ~/ (24 * 60 * 60));

/// Shown name of an invite-link use limit; `null` means no limit.
String _maxUsesLabel(AppLocalizations l10n, int? uses) =>
    uses == null ? l10n.inviteNoUseLimit : l10n.inviteUseCount(uses);

class _InviteOptionSheet<T> extends StatelessWidget {
  const _InviteOptionSheet({
    required this.title,
    required this.value,
    required this.options,
    required this.labelOf,
    required this.onSelected,
  });

  final String title;
  final T value;
  final List<CommunityInviteOption<T>> options;
  final String Function(T value) labelOf;
  final ValueChanged<T> onSelected;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(
              Grid.gutter,
              0,
              Grid.gutter,
              Grid.xxs,
            ),
            child: Text(title, style: context.textTheme.titleMedium),
          ),
          for (final option in options)
            AppListRow(
              key: Key('community-invite-option-${_optionKey(option.label)}'),
              title: labelOf(option.value),
              trailing: option.value == value
                  ? Icon(
                      LucideIcons.check,
                      size: 18,
                      color: context.colors.primary,
                    )
                  : null,
              onTap: () {
                onSelected(option.value);
                Navigator.of(context).pop();
              },
            ),
          const SizedBox(height: Grid.xxs),
        ],
      ),
    );
  }
}

class _InviteRowChevron extends StatelessWidget {
  const _InviteRowChevron();

  @override
  Widget build(BuildContext context) {
    return Icon(
      LucideIcons.chevronRight,
      size: 18,
      color: context.colors.onSurfaceVariant,
    );
  }
}

String _optionKey(String label) => label
    .toLowerCase()
    .replaceAll(RegExp(r'[^a-z0-9]+'), '-')
    .replaceAll(RegExp(r'^-|-$'), '');
