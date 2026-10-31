import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/relay.dart';

/// Profile reads that ask only for profiles that may have changed since the
/// relay last confirmed them.
void main() {
  const day = profileSinceMarginSeconds;

  test('unknown pubkeys share one full filter', () {
    final filters = NostrFilters.profilesSince(['a', 'b'], const {});

    expect(filters, hasLength(1));
    expect(filters.single.kinds, const [0]);
    expect(filters.single.authors, ['a', 'b']);
    expect(filters.single.since, isNull);
    expect(filters.single.limit, 2);
  });

  test('known pubkeys confirmed in one launch share one group, since = '
      'oldest confirmation minus a day', () {
    final filters = NostrFilters.profilesSince(
      ['u', 'k1', 'k2', 'k3'],
      const {'k1': 1000005, 'k2': 1000000, 'k3': 1000000},
    );

    // k1 is cut off at the only gap: within the cap each gap gets a cut.
    expect(filters, hasLength(3));
    expect(filters[0].authors, ['u']);
    expect(filters[0].since, isNull);
    expect(filters[1].authors, ['k2', 'k3']);
    expect(filters[1].since, 1000000 - day);
    expect(filters[1].limit, 2);
    expect(filters[2].authors, ['k1']);
    expect(filters[2].since, 1000005 - day);
  });

  test('equal confirmations never split', () {
    final known = {for (var i = 0; i < 50; i++) 'k$i': 2000000};

    final filters = NostrFilters.profilesSince(known.keys.toList(), known);

    expect(filters, hasLength(1));
    expect(filters.single.authors, hasLength(50));
    expect(filters.single.since, 2000000 - day);
  });

  test('groups are cut at the largest gaps within the filter cap', () {
    // 12 launches, a few pubkeys each; launches 3 and 7 follow long breaks.
    final known = <String, int>{};
    var at = 5000000;
    for (var launch = 0; launch < 12; launch++) {
      at += launch == 3 || launch == 7 ? 900000 : 1000 + launch;
      for (var i = 0; i < 4; i++) {
        known['l${launch}p$i'] = at + i; // same launch: seconds apart
      }
    }
    final pubkeys = ['u1', ...known.keys];

    final filters = NostrFilters.profilesSince(pubkeys, known);

    expect(filters, hasLength(kMaxFiltersPerReq));
    expect(filters.first.authors, ['u1']);
    final grouped = [for (final f in filters.skip(1)) ...f.authors!];
    expect(grouped, hasLength(known.length));
    expect(grouped.toSet(), known.keys.toSet());
    for (final filter in filters.skip(1)) {
      final times = [for (final key in filter.authors!) known[key]!];
      final min = times.reduce((a, b) => a < b ? a : b);
      expect(filter.since, min - day);
      expect(filter.limit, filter.authors!.length);
    }
    // The long breaks are group boundaries.
    final firsts = {for (final f in filters.skip(1)) f.authors!.first};
    expect(firsts, containsAll(['l3p0', 'l7p0']));
  });

  test('since never goes below zero', () {
    final filters = NostrFilters.profilesSince(['k'], const {'k': 10});

    expect(filters.single.since, 0);
  });
}
