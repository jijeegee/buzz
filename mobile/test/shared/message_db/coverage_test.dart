import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/message_db/coverage.dart';

CoverageCursor c(int at, [String id = 'm']) => CoverageCursor(at, id);
CoverageRange r(CoverageCursor lo, CoverageCursor hi) => CoverageRange(lo, hi);

void main() {
  group('CoverageCursor order', () {
    test('later seconds are later; within a second a smaller id is later', () {
      expect(c(10) < c(11), isTrue);
      // Relay pages newest first with ascending ids, so 'a' comes first.
      expect(c(10, 'b') < c(10, 'a'), isTrue);
      expect(CoverageCursor.start < c(0, 'zz'), isTrue);
      expect(c(10, 'a') < const CoverageCursor.endOfSecond(10), isTrue);
      expect(const CoverageCursor.endOfSecond(10) < c(11, 'zz'), isTrue);
    });
  });

  group('mergeCoverage', () {
    test('a range across a hole leaves both sides apart', () {
      final existing = [r(c(10), c(20)), r(c(40), c(50))];

      final result = mergeCoverage(existing, r(c(25), c(30)));

      expect(result.removed, isEmpty);
      expect(result.merged, r(c(25), c(30)));
    });

    test('ranges that share an endpoint merge', () {
      final result = mergeCoverage([r(c(10), c(20))], r(c(20), c(30)));

      expect(result.removed, [r(c(10), c(20))]);
      expect(result.merged, r(c(10), c(30)));
    });

    test('ranges that only come close stay split', () {
      // Nothing proves there is no event between (20, m) and (21, m).
      final result = mergeCoverage([r(c(10), c(20))], r(c(21), c(30)));

      expect(result.removed, isEmpty);
      expect(result.merged, r(c(21), c(30)));
    });

    test('an overlapping range extends the stored one', () {
      final result = mergeCoverage([r(c(10), c(20))], r(c(15), c(30)));

      expect(result.merged, r(c(10), c(30)));
    });

    test('a contained range changes nothing', () {
      final result = mergeCoverage([r(c(10), c(40))], r(c(15), c(30)));

      expect(result.removed, [r(c(10), c(40))]);
      expect(result.merged, r(c(10), c(40)));
    });

    test('a range bridging a hole merges both neighbours', () {
      final existing = [r(c(10), c(20)), r(c(30), c(40)), r(c(60), c(70))];

      final result = mergeCoverage(existing, r(c(20), c(30)));

      expect(result.removed, unorderedEquals(existing.take(2)));
      expect(result.merged, r(c(10), c(40)));
    });

    test('a growing range keeps absorbing ranges it now reaches', () {
      // Listed out of order: the first pass skips (5, 9) until the merged
      // range has grown down to it.
      final existing = [r(c(5), c(9)), r(c(9), c(12)), r(c(12), c(20))];

      final result = mergeCoverage(existing, r(c(15), c(25)));

      expect(result.removed, unorderedEquals(existing));
      expect(result.merged, r(c(5), c(25)));
    });
  });

  group('windowPageClaim', () {
    CoverageClaim? claim({
      required List<CoverageCursor> rows,
      CoverageCursor? start,
      bool newer = false,
      bool hasMore = true,
    }) => windowPageClaim(
      channelId: 'room',
      stream: CoverageStream.main,
      rows: rows,
      start: start,
      newer: newer,
      hasMore: hasMore,
    );

    test('the newest page covers its rows, not up to now', () {
      expect(claim(rows: [c(30), c(10), c(20)])?.range, r(c(10), c(30)));
    });

    test('a page without more rows reaches the first message', () {
      expect(
        claim(rows: [c(30), c(10)], hasMore: false)?.range,
        r(CoverageCursor.start, c(30)),
      );
    });

    test('an older page covers up to its request cursor', () {
      expect(
        claim(rows: [c(8), c(5)], start: c(10, 'x'))?.range,
        r(c(5), c(10, 'x')),
      );
      // An empty last page still proves nothing is older than the cursor.
      expect(
        claim(rows: [], start: c(10, 'x'), hasMore: false)?.range,
        r(CoverageCursor.start, c(10, 'x')),
      );
    });

    test('a newer page covers from its request cursor', () {
      expect(
        claim(rows: [c(12), c(15)], start: c(10, 'x'), newer: true)?.range,
        r(c(10, 'x'), c(15)),
      );
    });

    test('pages that prove nothing make no claim', () {
      expect(claim(rows: []), isNull);
      expect(claim(rows: [], hasMore: false), isNull);
      expect(claim(rows: [], start: c(10), newer: true), isNull);
    });

    test('an older page and a newer page from one anchor connect', () {
      const anchor = CoverageCursor(10, 'anchor');
      final older = claim(rows: [c(9), c(5)], start: anchor)!.range;
      final newer = claim(
        rows: [c(11), c(14)],
        start: anchor,
        newer: true,
      )!.range;

      expect(mergeCoverage([older], newer).merged, r(c(5), c(14)));
    });
  });
}
