part of '../invite_create_page.dart';

class _PersonInviteSection extends HookConsumerWidget {
  const _PersonInviteSection({required this.role});

  final CommunityMemberRole role;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final searchController = useTextEditingController();
    final searchFocusNode = useFocusNode();
    final isSearchEditing = useState(false);
    final candidatePubkey = useState<String?>(null);
    final selectedRole = useState(CommunityMemberRole.member);
    final isSubmitting = useState(false);
    final submitError = useState<String?>(null);

    final membership = ref.watch(communityMembershipProvider).asData?.value;
    final existingMemberPubkeys = {
      ...?membership?.pubkeys.map((pubkey) => pubkey.toLowerCase()),
    };
    final currentPubkey = ref.watch(myPubkeyProvider)?.toLowerCase();
    final l10n = context.l10n;

    useEffect(() {
      void handleFocusChange() {
        isSearchEditing.value = searchFocusNode.hasFocus;
      }

      searchFocusNode.addListener(handleFocusChange);
      return () => searchFocusNode.removeListener(handleFocusChange);
    }, [searchFocusNode]);

    void selectCandidate(String pubkey) {
      final normalized = pubkey.toLowerCase();
      if (normalized == currentPubkey) {
        candidatePubkey.value = null;
        submitError.value = l10n.inviteCannotInviteSelf;
        return;
      }
      if (existingMemberPubkeys.contains(normalized)) {
        candidatePubkey.value = null;
        submitError.value = l10n.inviteAlreadyMember;
        return;
      }
      candidatePubkey.value = normalized;
      submitError.value = null;
    }

    void handleInput(String value) {
      submitError.value = null;
      final pubkey = parseCommunityInvitePubkey(value);
      if (pubkey == null) {
        candidatePubkey.value = null;
      } else {
        selectCandidate(pubkey);
      }
    }

    Future<void> submit(CommunityInviteDirectoryUser invitee) async {
      if (isSubmitting.value) return;
      isSubmitting.value = true;
      submitError.value = null;
      try {
        await ref
            .read(communityInviteActionsProvider)
            .inviteMembers(
              pubkeys: [invitee.pubkey],
              role: role == CommunityMemberRole.owner
                  ? selectedRole.value
                  : CommunityMemberRole.member,
            );
        if (!context.mounted) return;
        candidatePubkey.value = null;
        searchController.clear();
        ref.invalidate(communityMembershipProvider);
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text(l10n.inviteInvitedToCommunity)));
      } catch (error) {
        if (context.mounted) {
          submitError.value = _inviteErrorMessage(error);
        }
      } finally {
        if (context.mounted) {
          isSubmitting.value = false;
        }
      }
    }

    final pubkey = candidatePubkey.value;
    final profileAsync = pubkey == null
        ? null
        : ref.watch(communityInviteProfileProvider(pubkey));

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
          child: SizedBox(
            key: const Key('community-invite-recipient-field'),
            width: double.infinity,
            height: kuzzSearchIdleFieldHeight,
            child: KuzzSearchField(
              fieldKey: const Key('community-invite-search'),
              controller: searchController,
              focusNode: searchFocusNode,
              hintText: l10n.inviteSearchNpub,
              iconColor: navigationPrimaryForeground(context),
              inputColor: navigationPrimaryForeground(context),
              placeholderColor: navigationSecondaryForeground(context),
              surfaceColor: navigationSearchSurface(context),
              isEditing: isSearchEditing.value,
              reduceMotion: MediaQuery.disableAnimationsOf(context),
              motionDuration: const Duration(milliseconds: 160),
              autocorrect: false,
              enableSuggestions: false,
              enabled: !isSubmitting.value,
              textInputAction: TextInputAction.done,
              onTap: searchFocusNode.requestFocus,
              onChanged: handleInput,
              onSubmitted: (value) {
                final pubkey = parseCommunityInvitePubkey(value);
                if (pubkey == null && value.trim().isNotEmpty) {
                  submitError.value = l10n.invitePasteValidNpub;
                } else if (pubkey != null) {
                  selectCandidate(pubkey);
                }
              },
            ),
          ),
        ),
        if (pubkey != null && profileAsync != null) ...[
          const SizedBox(height: Grid.xs),
          _InviteeResolutionCard(
            pubkey: pubkey,
            profileAsync: profileAsync,
            role: role,
            selectedRole: selectedRole.value,
            isSubmitting: isSubmitting.value,
            onRetry: () =>
                ref.invalidate(communityInviteProfileProvider(pubkey)),
            onRoleSelected: (value) => selectedRole.value = value,
            onInvite: submit,
          ),
        ],
        if (submitError.value case final error?) ...[
          const SizedBox(height: Grid.xxs),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
            child: Text(
              error,
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.error,
              ),
            ),
          ),
        ],
      ],
    );
  }
}

class _InviteeResolutionCard extends StatelessWidget {
  const _InviteeResolutionCard({
    required this.pubkey,
    required this.profileAsync,
    required this.role,
    required this.selectedRole,
    required this.isSubmitting,
    required this.onRetry,
    required this.onRoleSelected,
    required this.onInvite,
  });

  final String pubkey;
  final AsyncValue<CommunityInviteDirectoryUser?> profileAsync;
  final CommunityMemberRole role;
  final CommunityMemberRole selectedRole;
  final bool isSubmitting;
  final VoidCallback onRetry;
  final ValueChanged<CommunityMemberRole> onRoleSelected;
  final ValueChanged<CommunityInviteDirectoryUser> onInvite;

  @override
  Widget build(BuildContext context) {
    final l10n = context.l10n;
    return profileAsync.when(
      loading: () => AppListCard(
        children: [
          AppListRowRaw(
            key: Key('community-invite-resolving-$pubkey'),
            leading: SizedBox.square(
              dimension: 40,
              child: Center(
                child: KuzzLoadingIndicator(
                  size: 20,
                  semanticLabel: l10n.inviteResolvingProfile,
                ),
              ),
            ),
            title: Text(l10n.inviteResolvingProfile),
            subtitle: Text(shortPubkey(pubkey)),
          ),
        ],
      ),
      error: (_, _) => AppListCard(
        children: [
          AppListRow(
            key: Key('community-invite-resolution-error-$pubkey'),
            title: l10n.inviteResolveProfileFailed,
            subtitle: shortPubkey(pubkey),
            trailing: TextButton(
              onPressed: onRetry,
              child: Text(l10n.inviteRetry),
            ),
          ),
        ],
      ),
      data: (invitee) {
        if (invitee == null) {
          return AppListCard(
            children: [
              AppListRow(
                key: Key('community-invite-unresolved-$pubkey'),
                title: l10n.inviteInvalidNpub,
                subtitle: shortPubkey(pubkey),
              ),
            ],
          );
        }
        return AppListCard(
          key: const Key('community-invite-person-card'),
          dividerIndent: Grid.xs,
          children: [
            AppListRow(
              key: Key('community-invite-resolved-$pubkey'),
              title: shortPubkey(invitee.pubkey),
              trailing: FilledButton(
                key: const Key('community-invite-submit'),
                onPressed: isSubmitting ? null : () => onInvite(invitee),
                child: isSubmitting
                    ? KuzzLoadingIndicator(
                        size: 16,
                        semanticLabel: l10n.inviteInvitingPerson,
                      )
                    : Text(l10n.inviteInvite),
              ),
            ),
            if (role == CommunityMemberRole.owner)
              AppListRow(
                key: const Key('community-invite-role'),
                title: l10n.inviteRole,
                value: _roleLabel(l10n, selectedRole),
                trailing: const _InviteRowChevron(),
                onTap: isSubmitting
                    ? null
                    : () => _showInviteOptionSheet<CommunityMemberRole>(
                        context: context,
                        title: l10n.inviteRole,
                        value: selectedRole,
                        labelOf: (role) => _roleLabel(l10n, role),
                        options: const [
                          CommunityInviteOption(
                            label: 'Member',
                            value: CommunityMemberRole.member,
                          ),
                          CommunityInviteOption(
                            label: 'Admin',
                            value: CommunityMemberRole.admin,
                          ),
                        ],
                        onSelected: onRoleSelected,
                      ),
              ),
          ],
        );
      },
    );
  }
}

String _roleLabel(AppLocalizations l10n, CommunityMemberRole role) =>
    switch (role) {
      CommunityMemberRole.member => l10n.inviteRoleMember,
      CommunityMemberRole.admin => l10n.inviteRoleAdmin,
      CommunityMemberRole.owner => l10n.inviteRoleOwner,
    };
