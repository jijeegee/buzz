import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/app.dart';
import 'package:kuzz/features/activity/activity_page.dart';
import 'package:kuzz/features/age_gate/age_signal_provider.dart';
import 'package:kuzz/features/channels/channel_detail_page.dart';
import 'package:kuzz/features/channels/thread_detail_page.dart';
import 'package:kuzz/features/home/home_page.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/relay/user_stream.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'helpers/scripted_relay.dart';
import 'shared/community/community_storage_test.dart' show FakeSecureStorage;

/// Live REQ budget (PLANS/SUB_A0_REQ_BUDGET_TEST_PLAN §7): how many live
/// subscriptions the real [App] holds open at each checkpoint, checked
/// against the measured counts in `test-fixtures/live-req-budget.json`
/// (key `mobile`; desktop owns `desktop`).
///
/// The whole app runs connected: real auth, community, relay session and
/// feature providers, over [ScriptedRelay] (websocket REQs and HTTP
/// `/query`). The open set comes from the REQ/CLOSE/CLOSED log: `l-` ids are
/// live, `h-` ids one-shot; any open id that is not `l-` at a checkpoint
/// fails the test.
///
/// Regenerate after an intended change:
///   flutter test test/live_req_budget_test.dart \
///     --dart-define=KUZZ_PUSH_GATEWAY_URL=https://push.example \
///     --dart-define=KUZZ_UPDATE_REQ_BUDGET=true
/// (or `KUZZ_UPDATE_REQ_BUDGET=1` in the environment). Live counts and
/// signatures are replaced; launch ceilings keep the maximum of the old and
/// new values, so run it several times and delete a ceiling to lower it.

const _origin = 'https://relay.example';
final _me = nostr.Keys('1' * 64);
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';
const _relayKey =
    'cc00000000000000000000000000000000000000000000000000000000000003';
const _roomA = 'aaaaaaaa-0000-4000-8000-000000000001';
const _roomB = 'bbbbbbbb-0000-4000-8000-000000000002';
const _dm = 'dddddddd-0000-4000-8000-000000000004';
const _roomC10 = 'cccccccc-0000-4000-8000-000000000010';
const _roomC50 = 'cccccccc-0000-4000-8000-000000000050';
const _hiddenDm = 'dddddddd-0000-4000-8000-000000000005';
const _third =
    'ee00000000000000000000000000000000000000000000000000000000000005';
const _seededRooms = [_roomA, _roomB, _dm, _roomC10, _roomC50];

/// Tall enough that every seeded thread row of room C mounts.
const _viewSize = Size(800, 12000);

/// Quiet window: the log must not grow for this long before measuring.
const _quiet = Duration(seconds: 3);
const _step = Duration(milliseconds: 200);
const _settleCap = Duration(seconds: 30);

final bool _update =
    const bool.fromEnvironment('KUZZ_UPDATE_REQ_BUDGET') ||
    const {
      '1',
      'true',
    }.contains(Platform.environment['KUZZ_UPDATE_REQ_BUDGET']);

final _fixtureFile = File('../test-fixtures/live-req-budget.json');

Map<String, dynamic> _readFixture() => _fixtureFile.existsSync()
    ? jsonDecode(_fixtureFile.readAsStringSync()) as Map<String, dynamic>
    : <String, dynamic>{
        'description':
            'Measured live REQ budget per app (PLANS/SUB_A0 §7). Each app '
            'writes only its own key; target is documentation.',
        'desktop': <String, dynamic>{},
        'mobile': <String, dynamic>{},
        'target': <String, dynamic>{},
      };

final Map<String, dynamic> _expected =
    (_readFixture()['mobile'] as Map?)?.cast<String, dynamic>() ?? {};

/// What this run measured, merged into the fixture in update mode.
final _measured = <String, dynamic>{};

/// Owner rule (A4): 50 thread rows read their names in at most this many
/// HTTP queries.
const _ownerRows50NameReads = 2;

NostrEvent _ev(
  String id,
  int kind,
  String pubkey,
  int at,
  List<List<String>> tags, [
  String content = '',
]) => NostrEvent(
  id: id.padLeft(64, '0'),
  pubkey: pubkey,
  createdAt: at,
  kind: kind,
  tags: tags,
  content: content,
  sig: '',
);

List<NostrEvent> _room(String id, String name, int seq) => [
  _ev('${seq}01', 39000, _relayKey, 1000, [
    ['d', id],
    ['name', name],
    ['t', 'stream'],
    ['public'],
  ]),
  _ev('${seq}02', 39002, _relayKey, 1000, [
    ['d', id],
    ['p', _me.public, '', 'owner'],
    ['p', _other, '', 'member'],
  ]),
];

/// [heads] top-level messages in [roomId], each with one reply when
/// [replies]. Ids are `<seq><nnn>` heads and `<seq><nnn>1` replies.
List<NostrEvent> _messages(
  String roomId,
  int seq,
  int heads, {
  bool replies = true,
}) => [
  for (var n = 0; n < heads; n++) ...[
    _ev(
      '$seq${n.toString().padLeft(3, '0')}',
      40002,
      _other,
      2000 + n * 10,
      [
        ['h', roomId],
      ],
      'head $n of $roomId',
    ),
    if (replies)
      _ev(
        '$seq${n.toString().padLeft(3, '0')}1',
        40002,
        _other,
        2000 + n * 10 + 5,
        [
          ['h', roomId],
          [
            'e',
            '$seq${n.toString().padLeft(3, '0')}'.padLeft(64, '0'),
            '',
            'reply',
          ],
        ],
        'reply $n',
      ),
  ],
];

/// Every test seeds the same relay, so idle is the same in each.
List<NostrEvent> _seed() => [
  ..._room(_roomA, 'alpha', 1),
  ..._room(_roomB, 'bravo', 2),
  ..._room(_roomC10, 'charlie-10', 3),
  ..._room(_roomC50, 'charlie-50', 4),
  _ev('501', 39000, _relayKey, 1000, [
    ['d', _dm],
    ['name', 'DM'],
    ['t', 'dm'],
    ['hidden'],
    ['private'],
    ['p', _me.public],
    ['p', _other],
  ]),
  _ev('502', 39002, _relayKey, 1000, [
    ['d', _dm],
    ['p', _me.public, '', 'member'],
    ['p', _other, '', 'member'],
  ]),
  _ev('503', 0, _other, 1000, [], jsonEncode({'name': 'dana'})),
  ..._messages(_roomA, 6, 1, replies: false),
  ..._messages(_roomB, 7, 1),
  ..._messages(_dm, 8, 1, replies: false),
  ..._messages(_roomC10, 9, 10),
  ..._messages(_roomC50, 10, 50),
];

/// A second DM the viewer has hidden (kind:30622): left out of the list,
/// but a new message in it must still reach the app so it can come back.
List<NostrEvent> _hiddenDmSeed() => [
  _ev('511', 39000, _relayKey, 1000, [
    ['d', _hiddenDm],
    ['name', 'DM'],
    ['t', 'dm'],
    ['hidden'],
    ['private'],
    ['p', _me.public],
    ['p', _third],
  ]),
  _ev('512', 39002, _relayKey, 1000, [
    ['d', _hiddenDm],
    ['p', _me.public, '', 'member'],
    ['p', _third, '', 'member'],
  ]),
  _ev('513', 0, _third, 1000, [], jsonEncode({'name': 'erin'})),
  _ev('514', EventKind.dmVisibility, _me.public, 1500, [
    ['d', 'dm-visibility'],
    ['p', _me.public],
    ['h', _hiddenDm],
  ]),
  ..._messages(_hiddenDm, 11, 1, replies: false),
];

/// Mentions of the viewer in three rooms, for the activity inbox.
List<NostrEvent> _mentionSeed() => [
  for (final (n, roomId) in [(0, _roomA), (1, _roomB), (2, _roomC10)])
    _ev('12${n}00', 40002, _other, 3000 + n, [
      ['h', roomId],
      ['p', _me.public],
    ], 'mention $n'),
];

class _AllowedAgeSignalNotifier extends AgeSignalNotifier {
  @override
  AgeSignalState build() => AgeSignalState.allowed;

  @override
  Future<void> request() async {}
}

/// No lifecycle listener: a resume or connectivity event must not reconnect
/// mid-measurement (the session's socket factory is the scripted relay's
/// anyway, and [ScriptedRelay.connects] proves no second socket opened).
class _ResumedLifecycle extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;
}

const _pathProvider = MethodChannel('plugins.flutter.io/path_provider');
const _silentPlugins = [
  MethodChannel('app_badge_plus'),
  MethodChannel('com.llfbandit.app_links/events'),
  MethodChannel('com.llfbandit.app_links/messages'),
];

final _threadSummaryRow = find.byWidgetPredicate(
  (widget) => widget.runtimeType.toString() == '_ThreadSummaryRow',
);

/// The real [App], connected to a [ScriptedRelay] through the real
/// [RelaySessionNotifier] (its socket factory and `/query` client only).
class _Harness {
  _Harness({List<NostrEvent> extra = const []})
    : relay = ScriptedRelay(relayKey: _relayKey)
        ..events.addAll([..._seed(), ...extra]);

  final ScriptedRelay relay;
  late final ProviderContainer container;
  late final Directory _dir;

  Future<void> launch(WidgetTester tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = _viewSize;
    addTearDown(tester.view.reset);
    _dir = Directory.systemTemp.createTempSync('live_req_budget');
    final store = MessageStore(
      factory: () => databaseFactoryFfiNoIsolate,
      directory: () async => _dir.path,
    );
    // The head cache and other file users ask path_provider for a folder;
    // the other plugins only need an answer.
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(_pathProvider, (_) async => _dir.path);
    for (final channel in _silentPlugins) {
      messenger.setMockMethodCallHandler(channel, (_) async => null);
    }
    addTearDown(() {
      for (final channel in [_pathProvider, ..._silentPlugins]) {
        messenger.setMockMethodCallHandler(channel, null);
      }
    });
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final storage = CommunityStorage(secure: FakeSecureStorage());
    // A legacy community with a real key: without it myPubkey is empty and
    // every me-scoped subscription silently disappears from the count.
    final community = Community.create(
      name: 'Relay',
      relayUrl: _origin,
      pubkey: _me.public,
      nsec: _me.nsec,
    );
    await storage.save(community);
    await storage.saveActiveId(community.id);

    container = ProviderContainer(
      retry: (_, _) => null,
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        ageSignalProvider.overrideWith(_AllowedAgeSignalNotifier.new),
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        appLifecycleProvider.overrideWith(_ResumedLifecycle.new),
        messageStoreProvider.overrideWithValue(store),
        relayInfoFetcherProvider.overrideWithValue(
          (_) async => const RelayInfo(
            relaySelf: _relayKey,
            supportedExtensions: {'nip-cw-newer'},
          ),
        ),
        relaySessionProvider.overrideWith(
          () => RelaySessionNotifier(
            httpClient: relay.httpClient,
            socketFactory: relay.socketFactory,
          ),
        ),
      ],
    );
    await tester.pumpWidget(
      UncontrolledProviderScope(container: container, child: const App()),
    );
  }

  /// Tears the app down and drains its timers before the pending-timer check.
  Future<void> unmount(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox.shrink());
    container.dispose();
    await tester.pump(const Duration(seconds: 10));
    try {
      _dir.deleteSync(recursive: true);
    } on FileSystemException {
      // sqlite may still hold the file on Windows; the temp dir is disposable.
    }
  }

  /// Pumps in [_step]s until the log has not grown for [_quiet], at most
  /// [_settleCap]. Bounded: never `pumpAndSettle` (periodic timers).
  Future<void> settle(WidgetTester tester, String name) async {
    var quietFor = Duration.zero;
    var elapsed = Duration.zero;
    var seen = relay.frames.length;
    while (quietFor < _quiet) {
      if (elapsed >= _settleCap) {
        fail(
          '$name: the REQ log kept growing for $_settleCap; last frames:\n'
          '${relay.frames.skip(seen - 10 < 0 ? 0 : seen - 10).join('\n')}',
        );
      }
      await _advance(tester);
      elapsed += _step;
      if (relay.frames.length == seen) {
        quietFor += _step;
      } else {
        seen = relay.frames.length;
        quietFor = Duration.zero;
      }
    }
  }

  /// Settles, then checks the open live set against the fixture entry
  /// [name] (or records it in update mode).
  Future<Map<String, dynamic>> checkpoint(
    WidgetTester tester,
    String name,
  ) async {
    await settle(tester, name);
    final expected = (_expected[name] as Map?)?.cast<String, dynamic>();
    final expectedLive = expected?['live'] as int?;
    if (!_update && expectedLive != null) {
      // A late REQ (deferred work) may still be due: poll until the count
      // matches and holds for the quiet window, bounded by the cap.
      var elapsed = Duration.zero;
      var heldFor = Duration.zero;
      while (heldFor < _quiet && elapsed < _settleCap) {
        heldFor = relay.openLive.length == expectedLive
            ? heldFor + _step
            : Duration.zero;
        if (heldFor >= _quiet) break;
        await _advance(tester);
        elapsed += _step;
      }
    }
    final measured = <String, dynamic>{
      'live': relay.openLive.length,
      'signatures': relay.liveSignatures(),
    };
    expect(
      relay.connects,
      1,
      reason: '$name: the session opened another socket (reconnect)',
    );
    expect(
      relay.openUnclassified,
      isEmpty,
      reason: '$name: unclassified open subscription(s) (not `l-` live)',
    );
    _measured[name] = measured;
    if (_update) return measured;
    expect(
      expected,
      isNotNull,
      reason:
          'No `mobile.$name` in ${_fixtureFile.path}; run with '
          '--dart-define=KUZZ_UPDATE_REQ_BUDGET=true',
    );
    final want = (expected!['signatures'] as List).cast<String>();
    final got = measured['signatures'] as List<String>;
    expect(
      measured['live'],
      expectedLive,
      reason: '$name: open live subscriptions changed.\n${_diff(want, got)}',
    );
    expect(got, want, reason: '$name: live set changed.\n${_diff(want, got)}');
    return measured;
  }

  Future<void> open(WidgetTester tester, Finder target) async {
    await tester.tap(target.first);
    await tester.pump();
  }

  Future<void> back(WidgetTester tester, Type page) async {
    await Navigator.of(tester.element(find.byType(page).last)).maybePop();
    await tester.pump();
  }

  /// Sanity: the account bundle and the room bundle were sent, so the count
  /// is not low because the identity or the room list never loaded.
  void expectBundlesSent() {
    final reqs = [
      for (final frame in relay.frames)
        if (frame.type == 'REQ' && frame.subId.startsWith('l-')) frame,
    ];
    expect(
      reqs.any(
        (frame) => frame.filters.any(
          (filter) =>
              (filter['#h'] as List?)?.toSet().containsAll(_seededRooms) ==
                  true &&
              (filter['kinds'] as List).contains(EventKind.streamMessageV2),
        ),
      ),
      isTrue,
      reason: 'no room-bundle REQ names every seeded room',
    );
    expect(
      reqs.any(
        (frame) => frame.filters.any(
          (filter) =>
              (filter['authors'] as List?)?.single == _me.public &&
              !filter.keys.any((key) => key.startsWith('#')),
        ),
      ),
      isTrue,
      reason: 'no me-bundle authored REQ (is myPubkey set?)',
    );
  }
}

/// One [_step] of fake time, after letting real I/O (the message store's
/// file system calls) complete: under the widget tester's fake clock those
/// futures only resolve when the real event loop runs.
Future<void> _advance(WidgetTester tester) async {
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 2)),
  );
  await tester.pump(_step);
}

String _diff(List<String> want, List<String> got) {
  final added = [...got];
  final removed = <String>[];
  for (final signature in want) {
    if (!added.remove(signature)) removed.add(signature);
  }
  return [
    if (added.isNotEmpty) 'added:\n  ${added.join('\n  ')}',
    if (removed.isNotEmpty) 'removed:\n  ${removed.join('\n  ')}',
    'open now (${got.length}):\n  ${got.join('\n  ')}',
  ].join('\n');
}

/// Update mode: merges this test's measurements into the `mobile` key.
/// Read-modify-write right before writing, so the desktop key (written by
/// another runner) and the other mobile entries are kept. Only reached when
/// the test body completed.
void _writeMeasured(Iterable<String> keys) {
  if (!_update) return;
  final root = _readFixture();
  final mobile = (root['mobile'] as Map?)?.cast<String, dynamic>() ?? {};
  for (final key in keys) {
    final previous = (mobile[key] as Map?)?.cast<String, dynamic>();
    mobile[key] = {...?previous, ...(_measured[key] as Map)};
  }
  root['mobile'] = mobile;
  _fixtureFile.writeAsStringSync(
    '${const JsonEncoder.withIndent('  ').convert(root)}\n',
  );
  debugPrint('Wrote mobile ${keys.join(', ')} to ${_fixtureFile.path}');
}

/// HTTP `/query` calls that ask for thread names (40009).
int _threadNameHttpQueries(ScriptedRelay relay) => relay.httpQueries
    .where(
      (filters) => filters.any(
        (filter) =>
            (filter['kinds'] as List?)?.contains(EventKind.threadName) == true,
      ),
    )
    .length;

int _ceiling(String key, int measured) {
  final previous = (_expected['idle'] as Map?)?[key] as int?;
  return previous == null || measured > previous ? measured : previous;
}

void main() {
  sqfliteFfiInit();

  testWidgets('idle, rooms, thread and DM stay within the live REQ budget', (
    tester,
  ) async {
    final h = _Harness();
    await h.launch(tester);

    // Idle: Home, no room open.
    final idle = await h.checkpoint(tester, 'idle');
    expect(find.byType(HomePage), findsOneWidget);
    expect(find.byType(ChannelDetailPage), findsNothing);
    h.expectBundlesSent();
    final launch = {
      'launchOneShot': h.relay.oneShotReqSends,
      'launchLiveSends': h.relay.liveReqSends,
      // Information only: HTTP has no budget.
      'launchHttpQueries': h.relay.httpQueries.length,
    };
    if (_update) {
      idle['launchOneShot'] = _ceiling(
        'launchOneShot',
        launch['launchOneShot']!,
      );
      idle['launchLiveSends'] = _ceiling(
        'launchLiveSends',
        launch['launchLiveSends']!,
      );
      idle['launchHttpQueries'] = launch['launchHttpQueries'];
    } else {
      final ceilings = _expected['idle'] as Map;
      expect(
        launch['launchOneShot'],
        lessThanOrEqualTo(ceilings['launchOneShot'] as int),
        reason: 'launch sent more one-shot REQs than the ceiling',
      );
      expect(
        launch['launchLiveSends'],
        lessThanOrEqualTo(ceilings['launchLiveSends'] as int),
        reason: 'launch sent more live REQ frames than the ceiling (churn?)',
      );
    }
    debugPrint('launch: $launch');
    // What the launch one-shots asked for, so a ceiling change can be read.
    final launchOneShots = [
      for (final frame in h.relay.frames)
        if (frame.type == 'REQ' && frame.subId.startsWith('h-'))
          reqSignature(frame.filters),
    ]..sort();
    debugPrint('launch one-shots:\n  ${launchOneShots.join('\n  ')}');

    // Room A: a plain stream room with no thread rows.
    await h.open(tester, find.text('alpha'));
    await h.checkpoint(tester, 'roomOpen');
    expect(find.byType(ChannelDetailPage), findsOneWidget);
    expect(_threadSummaryRow, findsNothing);
    await h.back(tester, ChannelDetailPage);
    await h.settle(tester, 'back from room A');

    // Room B with its one thread row, panel closed.
    await h.open(tester, find.text('bravo'));
    await h.checkpoint(tester, 'roomOpenWithThreadRow');
    expect(_threadSummaryRow, findsOneWidget);

    // Room B's thread open.
    await h.open(tester, _threadSummaryRow);
    await h.checkpoint(tester, 'threadOpen');
    expect(find.byType(ThreadDetailPage), findsOneWidget);
    await h.back(tester, ThreadDetailPage);
    await h.settle(tester, 'back from thread');
    await h.back(tester, ChannelDetailPage);
    await h.settle(tester, 'back from room B');

    // The DM.
    await h.open(tester, find.text('dana'));
    await h.checkpoint(tester, 'dmOpen');
    expect(find.byType(ChannelDetailPage), findsOneWidget);

    await h.unmount(tester);
    _writeMeasured(const [
      'idle',
      'roomOpen',
      'roomOpenWithThreadRow',
      'threadOpen',
      'dmOpen',
    ]);
  });

  testWidgets('idle on a relay with the user stream in shadow', (tester) async {
    // docs/message-sync.md#live-req-budget: shadow adds the stream REQ next
    // to the chunks.
    final h = _Harness();
    h.relay
      ..userStreamClosed = null
      ..userStreamSnapshot = {
        'v': 1,
        'op': 'snapshot',
        'part': 1,
        'parts': 1,
        'mode': 'shadow',
        'epoch': 'e1',
        'kinds': userStreamFrozenKinds,
        'rooms': {_roomA: 1, _roomB: 1},
      };
    await h.launch(tester);
    await h.checkpoint(tester, 'idleStreamShadow');
    expect(find.byType(HomePage), findsOneWidget);
    await h.unmount(tester);
    _writeMeasured(const ['idleStreamShadow']);
  });

  testWidgets('idle with a hidden DM stays within the live REQ budget', (
    tester,
  ) async {
    final h = _Harness(extra: _hiddenDmSeed());
    await h.launch(tester);
    await h.checkpoint(tester, 'idleHiddenDm');
    expect(find.byType(HomePage), findsOneWidget);
    expect(find.text('erin'), findsNothing, reason: 'the hidden DM is listed');
    await h.unmount(tester);
    _writeMeasured(const ['idleHiddenDm']);
  });

  testWidgets('the activity inbox stays within the live REQ budget', (
    tester,
  ) async {
    final h = _Harness(extra: _mentionSeed());
    await h.launch(tester);
    await h.settle(tester, 'launch');
    // Not a tab in the app today; pushed so its subscriptions are counted.
    unawaited(
      Navigator.of(
        tester.element(find.byType(HomePage)),
      ).push(MaterialPageRoute<void>(builder: (_) => const ActivityPage())),
    );
    await tester.pump();
    await h.checkpoint(tester, 'activityInbox');
    expect(find.byType(ActivityPage), findsOneWidget);
    await h.unmount(tester);
    _writeMeasured(const ['activityInbox']);
  });

  for (final (rows, roomName) in [(10, 'charlie-10'), (50, 'charlie-50')]) {
    testWidgets('a stream room with $rows thread rows: per-row live cost', (
      tester,
    ) async {
      final h = _Harness();
      await h.launch(tester);
      await h.settle(tester, 'launch');
      await h.open(tester, find.text(roomName));
      final measured = await h.checkpoint(tester, 'rows$rows');
      // Read in the same synchronous step as the live set above.
      final mounted = _threadSummaryRow.evaluate().length;
      measured['seededRows'] = rows;
      measured['mountedRows'] = mounted;
      // A ceiling: update mode keeps the maximum over runs.
      final threadNameQueries = _threadNameHttpQueries(h.relay);
      final previousQueries =
          (_expected['rows$rows'] as Map?)?['http40009'] as int?;
      if (_update) {
        measured['http40009'] =
            previousQueries == null || threadNameQueries > previousQueries
            ? threadNameQueries
            : previousQueries;
      } else if (previousQueries != null) {
        expect(
          threadNameQueries,
          lessThanOrEqualTo(previousQueries),
          reason: 'rows$rows: more thread-name HTTP queries than the ceiling',
        );
      }
      expect(mounted, greaterThanOrEqualTo(10), reason: 'too few rows mounted');
      // Owner rule (A4), in both modes: a room with 10 or 50 thread rows holds
      // the room's own live REQs only, and 50 names come in at most 2 reads.
      final roomOpen = (_measured['roomOpen'] ?? _expected['roomOpen']) as Map?;
      if (roomOpen != null) {
        expect(
          measured['live'],
          roomOpen['live'],
          reason: 'rows$rows live must equal roomOpen live',
        );
      }
      if (rows == 50) {
        expect(
          threadNameQueries,
          lessThanOrEqualTo(_ownerRows50NameReads),
          reason: 'rows50: thread names must arrive in batched reads',
        );
      }
      if (rows == 50) {
        final ten = _measured['rows10'] as Map?;
        expect(ten, isNotNull, reason: 'the 10-row test must run first');
        final rowDelta = mounted - (ten!['mountedRows'] as int);
        final liveDelta = (measured['live'] as int) - (ten['live'] as int);
        expect(rowDelta, greaterThan(0), reason: 'no more rows mounted at 50');
        expect(
          liveDelta % rowDelta,
          0,
          reason: 'live delta $liveDelta is not a whole cost per row $rowDelta',
        );
        final perRow = liveDelta ~/ rowDelta;
        expect(perRow, 0, reason: 'a mounted thread row costs no live REQ');
        _measured['threadRows'] = {'livePerMountedRow': perRow};
        if (!_update) {
          expect(
            perRow,
            (_expected['threadRows'] as Map)['livePerMountedRow'],
            reason:
                'live subscriptions per mounted thread row changed: '
                '$liveDelta more live for $rowDelta more rows',
          );
        }
      }
      await h.unmount(tester);
      _writeMeasured(['rows$rows', if (rows == 50) 'threadRows']);
    });
  }
}
