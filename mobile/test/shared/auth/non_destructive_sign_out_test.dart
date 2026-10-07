import 'dart:async';

import 'package:buzz/shared/auth/auth_provider.dart';
import 'package:buzz/features/sign_in/signed_out_page.dart';
import 'package:buzz/shared/community/community.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/community/community_storage.dart';
import 'package:buzz/shared/community/community_token_session.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../community/community_storage_test.dart';

class _Harness {
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final keys = nostr.Keys.generate();
  final snapshots = <List<Community>>[];
  final ended = <String>[];
  String? failEnding;
  late ProviderContainer container;
  late Community first;
  late Community second;

  Future<void> start({bool token = false}) async {
    first = Community.create(
      name: 'First',
      relayUrl: 'https://first.test',
      pubkey: keys.public,
      nsec: keys.nsec,
      tokenAuth: token,
    );
    second = Community.create(
      name: 'Second',
      relayUrl: 'https://second.test',
      pubkey: keys.public,
      nsec: keys.nsec,
    );
    await storage.saveAll([first, second]);
    await storage.saveActiveId(first.id);
    openContainer();
    await container.read(authProvider.future);
    await container.read(communityListProvider.future);
  }

  void openContainer() {
    container = ProviderContainer(
      overrides: [
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((items) async {
          snapshots.add(List.of(items));
        }),
        communityPushLeaseRevocationEnqueuerProvider.overrideWithValue(
          (_) async => false,
        ),
        communityLogoutSessionEnderProvider.overrideWithValue((
          community, {
          required deviceOnly,
        }) async {
          if (community.id == failEnding) throw StateError('offline');
          ended.add(community.id);
        }),
      ],
    );
    final current = container;
    addTearDown(current.dispose);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'partial remote logout failure preserves identities and can retry',
    () async {
      final h = _Harness();
      await h.start();
      h.failEnding = h.second.id;
      await expectLater(
        h.container.read(authProvider.notifier).signOut(),
        throwsStateError,
      );
      expect(
        (await h.storage.loadAll()).every(
          (c) => !c.signedOut && c.nsec == h.keys.nsec,
        ),
        isTrue,
      );
      h.failEnding = null;
      await h.container.read(authProvider.notifier).signOut();
      expect(
        (await h.storage.loadAll()).every(
          (c) => c.signedOut && c.nsec == h.keys.nsec,
        ),
        isTrue,
      );
      expect(h.ended, [h.first.id, h.first.id, h.second.id]);
    },
  );

  testWidgets(
    'choose another retained account without unlocking; proof resumes only it',
    (tester) async {
      final h = _Harness();
      await h.start();
      await h.container.read(authProvider.notifier).signOut();
      await tester.pumpWidget(
        UncontrolledProviderScope(
          container: h.container,
          child: MaterialApp(
            home: Consumer(
              builder: (context, ref, _) {
                final state = ref.watch(authProvider).value;
                if (state == null) return const SizedBox();
                if (state.status == AuthStatus.authenticated) {
                  return const Text('Signed in');
                }
                return SignedOutPage(
                  community: state.community!,
                  cleanupError: state.logoutError,
                );
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('signed-out-community')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Second').last);
      await tester.pumpAndSettle();
      expect((await h.storage.loadAll()).every((c) => c.signedOut), isTrue);
      expect(
        h.container.read(authProvider).value?.status,
        AuthStatus.unauthenticated,
      );
      await tester.enterText(
        find.byKey(const Key('signed-out-private-key')),
        h.keys.nsec,
      );
      await tester.tap(find.byKey(const Key('signed-out-continue')));
      await tester.pumpAndSettle();
      expect(find.text('Signed in'), findsOneWidget);
      final saved = await h.storage.loadAll();
      expect(saved.first.signedOut, isTrue);
      expect(saved.last.signedOut, isFalse);
    },
  );

  test(
    'logout retains every key and membership; restart and switching stay signed out',
    () async {
      final h = _Harness();
      await h.start();
      await h.container.read(authProvider.notifier).signOut();
      final saved = await h.storage.loadAll();
      expect(saved.map((c) => c.id), [h.first.id, h.second.id]);
      expect(saved.every((c) => c.nsec == h.keys.nsec && c.signedOut), isTrue);
      expect(h.ended, [h.first.id, h.second.id]);
      expect(
        h.container.read(authProvider).value?.status,
        AuthStatus.unauthenticated,
      );
      expect(
        (await h.container.read(activeCommunityProvider.future))?.nsec,
        isNull,
      );
      await h.container.read(communityListProvider.notifier).syncSnapshot();
      expect(h.snapshots.last, isEmpty);

      h.openContainer();
      expect(
        (await h.container.read(authProvider.future)).status,
        AuthStatus.unauthenticated,
      );
      await h.container.read(communityListProvider.future);
      await h.container
          .read(communityListProvider.notifier)
          .switchCommunity(h.second.id);
      expect(
        (await h.container.read(authProvider.future)).status,
        AuthStatus.unauthenticated,
      );
    },
  );

  test(
    'only matching explicit key proof resumes one account with a new push lease',
    () async {
      final h = _Harness();
      await h.start();
      await h.container.read(authProvider.notifier).signOut();
      final auth = h.container.read(authProvider.notifier);
      await expectLater(
        auth.signInWithPrivateKey(nostr.Keys.generate().nsec),
        throwsStateError,
      );
      await expectLater(
        auth.authenticateWithCommunity(h.first),
        throwsStateError,
      );
      await auth.signInWithPrivateKey(h.keys.nsec);
      final saved = await h.storage.loadAll();
      expect(saved.first.signedOut, isFalse);
      expect(saved.last.signedOut, isTrue);
      expect(
        saved.first.pushLeaseInstallationId,
        isNot(h.first.pushLeaseInstallationId),
      );
      expect(saved.first.nsec, h.first.nsec);
      expect(
        await h.container
            .read(communityListProvider.notifier)
            .markPushLeaseAccepted(
              h.first.id,
              subscriptions: [],
              generation: 99,
              installationId: h.first.pushLeaseInstallationId,
            ),
        isFalse,
      );
      expect(
        (await h.storage.loadAll())
            .first
            .pushSubscriptionState
            .acceptedGeneration,
        isNull,
      );
    },
  );

  test(
    'delayed list updates cannot overwrite logout and a queued login is fenced',
    () async {
      final h = _Harness();
      await h.start();
      final block = Completer<void>();
      final entered = Completer<void>();
      final queue = h.container.read(communityTransitionProvider);
      final pending = queue.runExclusive(() async {
        entered.complete();
        await block.future;
      });
      await entered.future;
      final login = h.container
          .read(authProvider.notifier)
          .authenticateWithCommunity(h.first);
      final rejected = expectLater(login, throwsStateError);
      final logout = h.container.read(authProvider.notifier).signOut();
      block.complete();
      await Future.wait([pending, rejected, logout]);
      await h.container
          .read(communityListProvider.notifier)
          .renameCommunity(h.first.id, 'Renamed');
      await h.container
          .read(communityListProvider.notifier)
          .updateDesiredPushSubscriptions(h.first.id, []);
      expect((await h.storage.loadAll()).every((c) => c.signedOut), isTrue);
      expect(
        h.container.read(authProvider).value?.status,
        AuthStatus.unauthenticated,
      );
    },
  );

  test('token resume cannot replace the identity retained at logout', () async {
    final h = _Harness();
    await h.start(token: true);
    await h.container.read(authProvider.notifier).signOut();
    await expectLater(
      h.container
          .read(authProvider.notifier)
          .authenticateWithTokenSession(
            relayUrl: h.first.relayUrl,
            principalId: 'ab' * 32,
            resumeSignedOut: true,
          ),
      throwsStateError,
    );
    expect((await h.storage.loadAll()).first.pubkey, h.first.pubkey);
    expect((await h.storage.loadAll()).first.signedOut, isTrue);
  });

  test(
    'native cleanup failure stays signed out with an explicit retry state',
    () async {
      final h = _Harness();
      await h.start();
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      const channel = MethodChannel('buzz/push');
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(channel, (call) async {
        throw PlatformException(code: 'storage_unavailable');
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      await h.container.read(authProvider.notifier).signOut();
      expect(
        h.container.read(authProvider).value?.status,
        AuthStatus.unauthenticated,
      );
      expect(h.container.read(authProvider).value?.logoutError, isNotNull);
      expect((await h.storage.loadAll()).every((c) => c.signedOut), isTrue);
    },
  );
}
