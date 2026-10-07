part of '../settings_page.dart';

class _ConnectionSection extends ConsumerWidget {
  const _ConnectionSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final config = ref.watch(relayConfigProvider);
    final nsec = config.nsec;
    // A token community's account lives on the relay (see _AccountSection);
    // never expose a leftover local key for it.
    if (config.tokenAuth || nsec == null || nsec.isEmpty) {
      return const SizedBox.shrink();
    }
    return AppListCard(
      label: 'Connection',
      verticalPadding: Grid.twelve,
      children: [
        _IdentityRow(nsec: nsec),
        AppListRow(
          key: const Key('settings-google-key-backup'),
          icon: LucideIcons.shield,
          title: 'Google key backup',
          subtitle: 'Link or recover this signing identity',
          trailing: const _RowChevron(),
          onTap: () => unawaited(
            Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => TokenSignInPage(
                  backupOrigin: normalizeRelayOrigin(config.baseUrl),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _SignOutSection extends HookConsumerWidget {
  const _SignOutSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final error = useState<String?>(null);
    Future<void> signOut() async {
      if (busy.value) return;
      busy.value = true;
      error.value = null;
      try {
        await ref.read(authProvider.notifier).signOut();
        if (context.mounted) {
          Navigator.of(context).popUntil((route) => route.isFirst);
        }
      } catch (_) {
        if (context.mounted) {
          error.value = '로그아웃을 마치지 못했어요. 연결을 확인하고 다시 시도해 주세요.';
        }
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    return AppListCard(
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const Key('settings-sign-out'),
          icon: LucideIcons.logOut,
          title: busy.value ? '로그아웃 중…' : '로그아웃',
          subtitle: '계정과 커뮤니티는 유지됩니다',
          onTap: busy.value ? null : () => unawaited(signOut()),
        ),
        if (error.value != null)
          Semantics(liveRegion: true, child: Text(error.value!)),
      ],
    );
  }
}

/// Destructive removal is separate from ending a session.
class _RemoveCommunitySection extends ConsumerWidget {
  const _RemoveCommunitySection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    return AppListCard(
      verticalPadding: Grid.twelve,
      children: [
        ExpansionTile(
          key: const Key('settings-removal-management'),
          title: const Text('탈퇴·삭제 관리'),
          children: [
            AppListRow(
              key: const Key('settings-remove-community'),
              icon: LucideIcons.trash2,
              title: '이 기기에서 커뮤니티 제거',
              subtitle: '저장된 로그인 정보도 제거됩니다',
              onTap: () => _confirmRemoveCommunity(context, ref),
            ),
          ],
        ),
      ],
    );
  }
}

class _IdentityRow extends StatelessWidget {
  const _IdentityRow({required this.nsec});

  final String nsec;

  @override
  Widget build(BuildContext context) {
    final privHex = nostr.Nip19.decode(payload: nsec).data;
    final npub = privHex.isNotEmpty
        ? fullNpub(nostr.Keys(privHex).public)
        : null;

    // The full npub is the canonical copy/share form (never raw hex); an
    // invalid identity is surfaced as unavailable and never copied.
    return Semantics(
      button: true,
      label: 'Copy identity public key',
      value: npub ?? 'Identity unavailable',
      child: AppListRow(
        icon: LucideIcons.key,
        title: 'Identity (pubkey)',
        trailing: Icon(
          LucideIcons.copy,
          size: 18,
          color: context.colors.onSurfaceVariant,
        ),
        onTap: npub == null
            ? null
            : () async {
                await copyToClipboard(context, npub, message: 'Pubkey copied');
              },
      ),
    );
  }
}

void _confirmRemoveCommunity(BuildContext context, WidgetRef ref) {
  final usesPrivateKey = !ref.read(relayConfigProvider).tokenAuth;
  showBuzzDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('커뮤니티를 이 기기에서 제거할까요?'),
      content: Text(
        '이 기기의 커뮤니티 목록과 로그인 정보를 제거합니다. '
        '서버의 계정과 가입 정보는 삭제하지 않습니다.'
        '${usesPrivateKey ? '\n\n저장된 개인 키도 이 기기에서 제거됩니다. 개인 키 백업이나 Google 계정 복구가 준비되어 있는지 확인해 주세요.' : ''}',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(),
          child: const Text('취소'),
        ),
        FilledButton(
          onPressed: () async {
            Navigator.of(ctx).pop(); // close dialog
            try {
              final removed = await removeCommunityWithRelaySignOut(
                context,
                ({required deviceOnly}) => ref
                    .read(authProvider.notifier)
                    .removeActiveCommunity(deviceOnly: deviceOnly),
              );
              if (!removed) return;
            } catch (error) {
              if (!context.mounted) return;
              ScaffoldMessenger.of(context).showSnackBar(
                SnackBar(content: Text('커뮤니티를 제거하지 못했어요: $error')),
              );
              return;
            }
            if (!context.mounted) return;
            // Pop all pushed routes back to root so MaterialApp.home rebuilds
            // to onboarding when auth state changes.
            Navigator.of(context).popUntil((route) => route.isFirst);
          },
          child: const Text('제거'),
        ),
      ],
    ),
  );
}
