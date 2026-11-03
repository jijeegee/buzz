part of '../thread_detail_page.dart';

class _ThreadStickyDateUpdate {
  final StickyDateHeaderState state;
  final int? activeDayTimestamp;

  const _ThreadStickyDateUpdate({
    required this.state,
    required this.activeDayTimestamp,
  });

  static const hidden = _ThreadStickyDateUpdate(
    state: StickyDateHeaderState.hidden,
    activeDayTimestamp: null,
  );
}

class _ThreadStickyDateIndex {
  final Map<int, int> _dayTimestampByIndex = {};
  final Map<int, int> _dayStartByIndex = {};
  final Set<int> _dayHeaderIndices = {};
  final int messageCount;

  _ThreadStickyDateIndex({
    required TimelineMessage head,
    required List<TimelineMessage> replies,
  }) : messageCount = replies.length + 1 {
    // A stand-in head (root not loaded) has no time: it takes the first
    // reply's day instead of 1970.
    final headAt = head.pubkey.isEmpty && replies.isNotEmpty
        ? replies.first.createdAt
        : head.createdAt;
    final timestamps = [headAt, for (final reply in replies) reply.createdAt];
    var activeDayTimestamp = headAt;
    var activeDayStartIndex = 0;
    for (var index = 0; index < timestamps.length; index += 1) {
      final createdAt = timestamps[index];
      final startsDay =
          index == 0 || !isSameDay(timestamps[index - 1], createdAt);
      if (startsDay) {
        activeDayTimestamp = createdAt;
        activeDayStartIndex = index;
        _dayHeaderIndices.add(index);
      }
      _dayTimestampByIndex[index] = activeDayTimestamp;
      _dayStartByIndex[index] = activeDayStartIndex;
    }
  }

  _ThreadStickyDateUpdate resolve({
    required Iterable<ItemPosition> positions,
    required double viewportHeight,
    required double stickyTop,
    required double stickyHeaderHeight,
  }) {
    if (viewportHeight <= 0 || messageCount == 0) {
      return _ThreadStickyDateUpdate.hidden;
    }

    final visiblePositions = positions
        .where(
          (position) =>
              position.index < messageCount &&
              position.itemLeadingEdge < 1 &&
              position.itemTrailingEdge > 0,
        )
        .toList();
    if (visiblePositions.isEmpty) return _ThreadStickyDateUpdate.hidden;

    double physicalTop(ItemPosition position) =>
        viewportHeight * position.itemLeadingEdge;
    double physicalBottom(ItemPosition position) =>
        viewportHeight * position.itemTrailingEdge;

    final positionAtStickyTop = visiblePositions
        .where(
          (position) =>
              physicalTop(position) <= stickyTop &&
              physicalBottom(position) > stickyTop,
        )
        .firstOrNull;
    if (positionAtStickyTop == null) return _ThreadStickyDateUpdate.hidden;

    final activeDayTimestamp = _dayTimestampByIndex[positionAtStickyTop.index];
    final activeDayStartIndex = _dayStartByIndex[positionAtStickyTop.index];
    if (activeDayTimestamp == null || activeDayStartIndex == null) {
      return _ThreadStickyDateUpdate.hidden;
    }

    final activeHeaderPosition = visiblePositions
        .where((position) => position.index == activeDayStartIndex)
        .firstOrNull;
    final firstVisibleIndex = visiblePositions
        .map((position) => position.index)
        .reduce((left, right) => left < right ? left : right);
    final activeHeaderHasCrossed = activeHeaderPosition != null
        ? physicalTop(activeHeaderPosition) <= stickyTop
        : activeDayStartIndex < firstVisibleIndex;
    if (!activeHeaderHasCrossed) return _ThreadStickyDateUpdate.hidden;

    double? nextHeaderTop;
    for (final position in visiblePositions) {
      if (!_dayHeaderIndices.contains(position.index) ||
          position.index <= activeDayStartIndex) {
        continue;
      }
      final top = physicalTop(position);
      if (top <= stickyTop || (nextHeaderTop != null && top >= nextHeaderTop)) {
        continue;
      }
      nextHeaderTop = top;
    }

    final rawTranslateY = nextHeaderTop == null
        ? 0.0
        : math.min(0.0, nextHeaderTop - stickyTop - stickyHeaderHeight - 5);
    final translateY = rawTranslateY
        .clamp(-(stickyHeaderHeight + 5), 0.0)
        .toDouble();
    return _ThreadStickyDateUpdate(
      state: StickyDateHeaderState(
        label: formatDayHeading(activeDayTimestamp),
        translateY: (translateY * 2).round() / 2,
      ),
      activeDayTimestamp: activeDayTimestamp,
    );
  }
}
