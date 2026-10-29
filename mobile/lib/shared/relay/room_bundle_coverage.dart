/// Coverage of the device message store from live room subscriptions, and
/// catching the store up on what it missed while away. Mirrors desktop's
/// `relayStoreCoverage.ts`. Rules: docs/message-sync.md#coverage.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../message_store/message_store.dart';
import 'nostr_models.dart';

export 'store_heads.dart';

/// Same margin as desktop reconnect repair: relay future tolerance plus
/// storage fence.
const storeResumeLookbackSecs = 900 + 960 + 5;

/// A live REQ records its coverage at most this often.
const storeMarkIntervalSecs = 60;
const storeCatchUpPageLimit = 500;

/// Beyond this the gap is left for the room to fill when it is opened.
const storeCatchUpMaxPages = 4;
const storeCatchUpMaxAgeSecs = 7 * 24 * 60 * 60;

/// Rooms one keyset request may name (desktop `MAX_REPAIR_CHANNELS`).
const _maxRooms = 128;

typedef StoreScope = ({MessageStore store, MessageStoreLease lease});

class _Live {
  _Live(this.generation, this.liveAt, this.scope, this.captured)
    : markedAt = liveAt;

  final int generation;
  final int liveAt;
  final StoreScope scope;

  /// The store capture taken at EOSE: spans recorded from it are refused
  /// for rooms forgotten after it (docs/message-sync.md#forgetting-a-room).
  final int captured;
  int markedAt;

  /// Completes once the catch-up begun at this EOSE has ended.
  Future<void> settled = Future.value();
}

/// Room ids of a live REQ carrying every channel event of explicit rooms.
List<String>? storeCoveredRooms(NostrFilter filter) {
  final roomIds = filter.tags['#h'];
  if (roomIds == null ||
      roomIds.isEmpty ||
      roomIds.length > _maxRooms ||
      !EventKind.channelEventKinds.every(filter.kinds.contains)) {
    return null;
  }
  return roomIds;
}

class RoomBundleCoverage {
  RoomBundleCoverage({
    required this.scope,
    required this.query,
    int Function()? nowSecs,
  }) : _nowSecs =
           nowSecs ?? (() => DateTime.now().millisecondsSinceEpoch ~/ 1000);

  /// The store and account events are received for now, if any.
  final StoreScope? Function() scope;

  /// A `/query` request; its event results are stored by the session.
  final Future<Object?> Function(List<Map<String, dynamic>> filters) query;
  final int Function() _nowSecs;
  final _live = Expando<_Live>();
  Future<void> _catchUps = Future.value();

  /// A covered live REQ (keyed by [subscription]) reached EOSE on connection
  /// [generation]: from now on it receives every event of its rooms. Catch
  /// the stored rooms up from where they stopped, into the store only.
  void noteEose(Object subscription, NostrFilter filter, int generation) {
    final roomIds = storeCoveredRooms(filter);
    final current = scope();
    if (roomIds == null || current == null) return;
    final captured = current.store.capture(current.lease);
    if (captured == null) return;
    final liveAt = _nowSecs();
    final at = captured.sequence;
    final live = _Live(generation, liveAt, current, at);
    _live[subscription] = live;
    current.store.extendCoverage(
      current.lease,
      roomIds,
      liveAt,
      liveAt,
      captured: at,
    );
    _catchUps = _catchUps.then(
      (_) => catchUp(current, roomIds, liveAt, captured: at).catchError((
        Object error,
      ) {
        debugPrint('[MessageStore] catch-up stopped: $error');
        return false;
      }),
    );
    live.settled = _catchUps;
  }

  /// Completes once [subscription]'s catch-up since its EOSE on connection
  /// [generation] has ended; `null` when it is not live there.
  Future<void>? settledOf(Object subscription, int generation) {
    final live = _live[subscription];
    return live != null && live.generation == generation ? live.settled : null;
  }

  /// When [subscription] reached EOSE on connection [generation], if it has
  /// been receiving every event of its rooms since.
  int? liveAtOf(Object subscription, int generation) {
    final live = _live[subscription];
    return live != null && live.generation == generation ? live.liveAt : null;
  }

  /// The coverage of [subscription]'s EOSE on connection [generation], as a
  /// part of a head token (docs/message-sync.md#head-spans), or `null`. Each
  /// is made at one EOSE and never reused: while [holds], the REQ has been
  /// receiving every event of its rooms since.
  Object? liveTokenOf(Object subscription, int generation) {
    final live = _live[subscription];
    return live != null && live.generation == generation ? live : null;
  }

  /// Whether [token] from [liveTokenOf] is still [subscription]'s coverage on
  /// connection [generation].
  bool holds(Object subscription, Object token, int generation) =>
      identical(_live[subscription], token) &&
      (token as _Live).generation == generation;

  /// When the REQ of [token] reached EOSE.
  int liveAtOfToken(Object token) => (token as _Live).liveAt;

  /// The relay closed [subscription]: not receiving until its retry reaches
  /// EOSE again.
  void noteClosed(Object subscription) => _live[subscription] = null;

  /// An event arrived on [generation]: the REQ was still live, so extend.
  void noteEvent(Object subscription, NostrFilter filter, int generation) {
    final live = _live[subscription];
    if (live == null || live.generation != generation) return;
    final now = _nowSecs();
    if (now - live.markedAt < storeMarkIntervalSecs) return;
    final roomIds = storeCoveredRooms(filter);
    if (roomIds == null) return;
    live.markedAt = now;
    live.scope.store.extendCoverage(
      live.scope.lease,
      roomIds,
      live.liveAt,
      now,
      captured: live.captured,
    );
  }

  /// Fetch what the store is missing for [roomIds] between their last
  /// coverage and [liveAt]. Rooms never covered are skipped; a gap too old
  /// or too large is left to opening the room. [captured]: the store
  /// capture of the REQ's EOSE (default: when the span is recorded).
  @visibleForTesting
  Future<bool> catchUp(
    StoreScope scope,
    List<String> roomIds,
    int liveAt, {
    int? captured,
  }) async {
    // Spans opened at liveAt (this REQ's own) may be written already.
    final points = await scope.store.resumePoints(scope.lease, roomIds, liveAt);
    final covered = roomIds.where(points.containsKey).toList();
    if (covered.isEmpty) return false;
    final oldest = covered
        .map((roomId) => points[roomId]!)
        .reduce((a, b) => a < b ? a : b);
    final since = oldest - storeResumeLookbackSecs < 0
        ? 0
        : oldest - storeResumeLookbackSecs;
    if (since < liveAt - storeCatchUpMaxAgeSecs) return false;
    // The first page starts at the relay's newest, not at this device's
    // clock: rows the relay stamped after liveAt but sent before EOSE are
    // fetched too.
    int? until;
    String? beforeId;
    for (var page = 0; page < storeCatchUpMaxPages; page++) {
      final decoded = await query([
        {
          '#h': covered,
          'kinds': EventKind.channelEventKinds,
          'since': since,
          'limit': storeCatchUpPageLimit,
          'until': ?until,
          'before_id': ?beforeId,
        },
      ]);
      if (decoded is! List) return false;
      final terminal = decoded.isEmpty ? null : decoded.last;
      if (decoded.length < storeCatchUpPageLimit || terminal == null) {
        scope.store.extendCoverage(
          scope.lease,
          covered,
          since,
          liveAt,
          captured: captured,
        );
        return true;
      }
      if (terminal is! Map) return false;
      final createdAt = terminal['created_at'];
      final id = terminal['id'];
      if (createdAt is! int || id is! String) return false;
      if (createdAt == until && id == beforeId) return false;
      until = createdAt;
      beforeId = id;
    }
    return false;
  }
}
