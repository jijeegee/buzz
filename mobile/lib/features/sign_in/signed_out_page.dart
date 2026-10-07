import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/auth_provider.dart';
import '../../shared/community/community.dart';
import '../../shared/community/community_provider.dart';
import '../../shared/theme/theme.dart';
import 'token_sign_in_page.dart';

/// Reauthentication only: stored identities never unlock by selecting a row.
class SignedOutPage extends HookConsumerWidget {
  const SignedOutPage({super.key, required this.community, this.cleanupError});

  final Community community;
  final String? cleanupError;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final key = useTextEditingController();
    final busy = useState(false);
    final error = useState<String?>(null);
    final communities = ref.watch(communityListProvider).value ?? [];
    final retained = communities.where((item) => item.signedOut).toList();
    useEffect(() {
      key.clear();
      error.value = null;
      return null;
    }, [community.id]);
    Future<void> selectCommunity(String? id) async {
      if (id == null || busy.value) return;
      busy.value = true;
      try {
        await ref.read(communityListProvider.notifier).switchCommunity(id);
      } catch (_) {
        if (context.mounted) error.value = '계정을 선택하지 못했어요. 다시 시도해 주세요.';
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    final chooser = cleanupError == null && retained.length > 1
        ? AppBar(
            automaticallyImplyLeading: false,
            title: DropdownButton<String>(
              key: const Key('signed-out-community'),
              value: community.id,
              isExpanded: true,
              items: [
                for (final item in retained)
                  DropdownMenuItem(value: item.id, child: Text(item.name)),
              ],
              onChanged: busy.value
                  ? null
                  : (id) => unawaited(selectCommunity(id)),
            ),
          )
        : null;
    if (cleanupError == null &&
        (community.tokenAuth || community.googleBackupAccountId != null)) {
      return Scaffold(
        appBar: chooser,
        body: Column(
          children: [
            if (error.value != null)
              Semantics(liveRegion: true, child: Text(error.value!)),
            Expanded(
              child: TokenSignInPage(
                key: ValueKey(community.id),
                lockedOrigin: community.tokenAuth ? community.relayUrl : null,
                backupOrigin: community.tokenAuth ? null : community.relayUrl,
                resumeSignedOut: true,
              ),
            ),
          ],
        ),
      );
    }
    Future<void> submit() async {
      if (busy.value) return;
      busy.value = true;
      error.value = null;
      try {
        final auth = ref.read(authProvider.notifier);
        if (cleanupError != null) {
          await auth.signOut();
        } else {
          await auth.signInWithPrivateKey(key.text);
        }
      } catch (_) {
        if (context.mounted) {
          error.value = cleanupError != null
              ? '로그아웃을 마치지 못했어요. 다시 시도해 주세요.'
              : '이 계정의 개인 키를 확인해 주세요.';
        }
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    return Scaffold(
      appBar: chooser,
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            Text('다시 로그인', style: context.textTheme.headlineSmall),
            const SizedBox(height: Grid.sm),
            Text(cleanupError ?? '${community.name}\n계정과 커뮤니티는 그대로 있어요.'),
            if (cleanupError == null) ...[
              const SizedBox(height: Grid.sm),
              TextField(
                key: const Key('signed-out-private-key'),
                controller: key,
                enabled: !busy.value,
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                decoration: const InputDecoration(labelText: '개인 키 (nsec)'),
                onSubmitted: (_) => unawaited(submit()),
              ),
            ],
            const SizedBox(height: Grid.sm),
            FilledButton(
              key: const Key('signed-out-continue'),
              onPressed: busy.value ? null : () => unawaited(submit()),
              child: Text(cleanupError == null ? '로그인' : '로그아웃 마무리'),
            ),
            if (error.value != null)
              Semantics(liveRegion: true, child: Text(error.value!)),
          ],
        ),
      ),
    );
  }
}
