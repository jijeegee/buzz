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
///
/// Every community resumes through Google. A key-based community that was
/// never linked links the key it retained at sign-out to the Google account
/// used here (see `GoogleKeyBackupService`); no key is ever typed or shown.
class SignedOutPage extends HookConsumerWidget {
  const SignedOutPage({super.key, required this.community, this.cleanupError});

  final Community community;
  final String? cleanupError;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final busy = useState(false);
    final error = useState<String?>(null);
    final communities = ref.watch(communityListProvider).value ?? [];
    final retained = communities.where((item) => item.signedOut).toList();
    useEffect(() {
      error.value = null;
      return null;
    }, [community.id]);
    Future<void> selectCommunity(String? id) async {
      if (id == null || busy.value) return;
      busy.value = true;
      try {
        await ref.read(communityListProvider.notifier).switchCommunity(id);
      } catch (_) {
        if (context.mounted) {
          error.value = 'Couldn’t switch communities. Try again.';
        }
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
    if (cleanupError == null) {
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
    Future<void> finishSignOut() async {
      if (busy.value) return;
      busy.value = true;
      error.value = null;
      try {
        await ref.read(authProvider.notifier).signOut();
      } catch (_) {
        if (context.mounted) {
          error.value = 'Couldn’t finish signing out. Try again.';
        }
      } finally {
        if (context.mounted) busy.value = false;
      }
    }

    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(Grid.sm),
          children: [
            Text('Finishing sign-out', style: context.textTheme.headlineSmall),
            const SizedBox(height: Grid.sm),
            Text(cleanupError!),
            const SizedBox(height: Grid.sm),
            FilledButton(
              key: const Key('signed-out-continue'),
              onPressed: busy.value ? null : () => unawaited(finishSignOut()),
              child: const Text('Finish signing out'),
            ),
            if (error.value != null)
              Semantics(liveRegion: true, child: Text(error.value!)),
          ],
        ),
      ),
    );
  }
}
