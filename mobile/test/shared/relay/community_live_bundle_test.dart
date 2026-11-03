import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/community_live_bundle.dart';
import 'package:kuzz/shared/relay/relay.dart';

NostrFilter _filter(Map json) => NostrFilter(
  kinds: (json['kinds'] as List).cast<int>(),
  authors: (json['authors'] as List?)?.cast<String>(),
  limit: json['limit'] as int,
  since: json['since'] as int?,
  tags: {
    for (final entry in json.entries)
      if ((entry.key as String).startsWith('#'))
        entry.key as String: (entry.value as List).cast<String>(),
  },
);

class _Req {
  _Req(this.filters, this.onEvent, this.onStatus, this.onGone);

  final List<NostrFilter> filters;
  final void Function(NostrEvent) onEvent;
  final void Function(RelaySubscriptionStatus) onStatus;
  final void Function() onGone;
  final updates = <({int at, List<NostrFilter> filters})>[];
  final opened = Completer<LiveFiltersHandle>();
  bool closed = false;
}

String _key(int i) => i.toRadixString(16).padLeft(64, '0');

List<String>? _presenceOf(List<NostrFilter> filters) {
  for (final filter in filters) {
    if (filter.kinds.first == 20001) return filter.authors;
  }
  return null;
}

NostrEvent _event(
  String id,
  int kind,
  String pubkey, [
  List<List<String>>? tags,
]) => NostrEvent(
  id: id,
  pubkey: pubkey,
  createdAt: 1,
  kind: kind,
  tags: tags ?? const [],
  content: '',
  sig: '',
);

void main() {
  final fixtures =
      jsonDecode(
            File(
              '../test-fixtures/community-live-bundle.json',
            ).readAsStringSync(),
          )
          as Map<String, dynamic>;

  test('shared fixture: rows match desktop', () {
    expect([
      for (final row in fixtures['rows'] as List)
        if (row['mobile'] == true) row['name'],
    ], mobileCommunityRows.map((row) => row.name).toList());
    for (final row in fixtures['rows'] as List) {
      final filter =
          communityRowFilters[CommunityRow.values.byName(
            row['name'] as String,
          )]!;
      expect(filter.toJson(), _filter(row['filter'] as Map).toJson());
    }
  });

  for (final fixture in fixtures['scopeCases'] as List) {
    test('shared fixture: ${jsonEncode(fixture['filter'])}', () {
      expect(
        communityScopeOf(_filter(fixture['filter'] as Map))?.name,
        fixture['mobile'],
      );
    });
  }

  late List<_Req> reqs;

  CommunityLiveBundle makeBundle(FakeAsync async) {
    reqs = [];
    return CommunityLiveBundle(
      open: (filters, onEvent, onStatus, onGone) {
        final req = _Req(filters, onEvent, onStatus, onGone);
        reqs.add(req);
        return req.opened.future;
      },
      nowMs: () => async.elapsed.inMilliseconds,
    );
  }

  void ready(_Req req, FakeAsync async) {
    req.onStatus(RelaySubscriptionStatus.ready);
    req.opened.complete(
      LiveFiltersHandle(
        close: () => req.closed = true,
        setFilters: (next) async {
          req.updates.add((at: async.elapsed.inMilliseconds, filters: next));
        },
      ),
    );
    async.flushMicrotasks();
  }

  final status = communityRowFilters[CommunityRow.status]!;
  final presence = communityRowFilters[CommunityRow.presence]!;

  test('one REQ, opened 250 ms after the first listener', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      bundle.subscribe(status, (_) {});
      bundle.addPresenceAuthors([_key(1).toUpperCase()]);
      async.elapse(const Duration(milliseconds: 249));
      expect(reqs, isEmpty);
      async.elapse(const Duration(milliseconds: 1));
      expect(reqs, hasLength(1));
      expect(
        [for (final f in reqs.single.filters) f.toJson()],
        [
          status.toJson(),
          NostrFilter(
            kinds: const [20001],
            authors: [_key(1)],
            limit: 0,
          ).toJson(),
        ],
      );
    });
  });

  test('presence is omitted while the author set is empty', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      bundle.subscribe(status, (_) {});
      async.elapse(const Duration(milliseconds: 250));
      expect(
        [for (final f in reqs.single.filters) f.toJson()],
        [status.toJson()],
      );
    });
  });

  test('listeners hear what they match, once', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      final statuses = <String>[];
      final dots = <String>[];
      bundle.subscribe(status, (e) => statuses.add(e.id));
      bundle.subscribe(presence, (e) => dots.add(e.id));
      async.elapse(const Duration(milliseconds: 250));
      ready(reqs.single, async);
      final req = reqs.single;
      req.onEvent(
        _event('s1', 30315, _key(1), [
          ['d', 'general'],
        ]),
      );
      req.onEvent(
        _event('s1', 30315, _key(1), [
          ['d', 'general'],
        ]),
      );
      req.onEvent(_event('p1', 20001, _key(1)));
      expect(statuses, ['s1']);
      expect(dots, ['p1']);
    });
  });

  test('a filter with authors is refused', () async {
    final bundle = CommunityLiveBundle(
      open: (_, _, _, _) => Completer<LiveFiltersHandle>().future,
    );
    await expectLater(
      bundle.subscribe(
        NostrFilter(kinds: const [20001], authors: [_key(1)], limit: 0),
        (_) {},
      ),
      throwsArgumentError,
    );
  });

  test('200 adds over 90 s: at most one re-REQ per 30 s, first at 250 ms', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      bundle.subscribe(presence, (_) {});
      async.elapse(const Duration(milliseconds: 250));
      ready(reqs.single, async);
      final start = async.elapsed.inMilliseconds;
      for (var i = 0; i < 200; i++) {
        bundle.addPresenceAuthors([_key(100 + i)]);
        async.elapse(const Duration(milliseconds: 450));
      }
      async.elapse(const Duration(seconds: 30));
      final updates = reqs.single.updates;
      expect(updates.first.at - start, 250);
      for (var i = 1; i < updates.length; i++) {
        expect(updates[i].at - updates[i - 1].at, greaterThanOrEqualTo(30000));
      }
      expect(updates.length, lessThanOrEqualTo(5));
      expect(_presenceOf(updates.last.filters), hasLength(200));
      async.elapse(const Duration(seconds: 60));
      final before = updates.length;
      bundle.addPresenceAuthors([_key(9999)]);
      async.elapse(const Duration(milliseconds: 249));
      expect(updates, hasLength(before));
      async.elapse(const Duration(milliseconds: 1));
      expect(updates, hasLength(before + 1));
    });
  });

  test('adds before the opening EOSE follow it at 250 ms', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      bundle.subscribe(presence, (_) {});
      bundle.addPresenceAuthors([_key(1)]);
      async.elapse(const Duration(milliseconds: 250));
      expect(_presenceOf(reqs.single.filters), [_key(1)]);
      bundle.addPresenceAuthors([_key(2)]);
      async.elapse(const Duration(seconds: 5));
      expect(reqs.single.updates, isEmpty);
      ready(reqs.single, async);
      async.elapse(const Duration(milliseconds: 250));
      expect(_presenceOf(reqs.single.updates.single.filters), [
        _key(1),
        _key(2),
      ]);
    });
  });

  test('the set is capped at 1000', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      final refused = bundle.addPresenceAuthors([
        for (var i = 1; i <= presenceAuthorCap + 5; i++) _key(i),
      ]);
      expect(refused, hasLength(5));
      expect(bundle.tracksPresenceOf(_key(1)), isTrue);
      expect(bundle.tracksPresenceOf(_key(presenceAuthorCap + 1)), isFalse);
    });
  });

  test('reset clears the set and asks listeners to add keys again', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      var resets = 0;
      bundle.subscribe(presence, (_) {}, onReset: () => resets++);
      bundle.addPresenceAuthors([_key(1)]);
      async.elapse(const Duration(milliseconds: 250));
      ready(reqs.single, async);
      bundle.reset();
      expect(resets, 1);
      expect(reqs.single.closed, isTrue);
      bundle.addPresenceAuthors([_key(2)]);
      async.elapse(const Duration(milliseconds: 250));
      expect(reqs, hasLength(2));
      expect(_presenceOf(reqs.last.filters), [_key(2)]);
    });
  });

  test('a dropped REQ reopens with backoff; retrying, then ready', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      final statuses = <RelaySubscriptionStatus>[];
      bundle.subscribe(status, (_) {}, onStatus: statuses.add);
      async.elapse(const Duration(milliseconds: 250));
      ready(reqs.single, async);
      reqs.single.onGone();
      expect(statuses, [
        RelaySubscriptionStatus.ready,
        RelaySubscriptionStatus.retrying,
      ]);
      async.elapse(const Duration(milliseconds: 999));
      expect(reqs, hasLength(1));
      async.elapse(const Duration(milliseconds: 1));
      expect(reqs, hasLength(2));
      reqs[1].opened.completeError(Exception('open failed'));
      async.flushMicrotasks();
      async.elapse(const Duration(milliseconds: 1999));
      expect(reqs, hasLength(2));
      async.elapse(const Duration(milliseconds: 1));
      expect(reqs, hasLength(3));
      ready(reqs[2], async);
      expect(statuses, [
        RelaySubscriptionStatus.ready,
        RelaySubscriptionStatus.retrying,
        RelaySubscriptionStatus.retrying,
        RelaySubscriptionStatus.ready,
      ]);
    });
  });

  test('closes one task after the last stop; a quick resubscribe keeps it', () {
    fakeAsync((async) {
      final bundle = makeBundle(async);
      late void Function() stop;
      bundle.subscribe(status, (_) {}).then((s) => stop = s);
      async.elapse(const Duration(milliseconds: 250));
      ready(reqs.single, async);
      stop();
      bundle.subscribe(status, (_) {}).then((s) => stop = s);
      async.flushMicrotasks();
      expect(reqs.single.closed, isFalse);
      stop();
      async.flushMicrotasks();
      expect(reqs.single.closed, isTrue);
    });
  });
}
