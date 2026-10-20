import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:path_provider/path_provider.dart';

/// Default size budget of the media disk cache.
const mediaDiskCacheMaxBytes = 200 * 1024 * 1024;

final _contentAddressedPath = RegExp(
  r'^/media/[0-9a-f]{64}(\.[A-Za-z0-9.]+)?$',
);

/// Persistent cache of media bytes, so avatars and thumbnails are not
/// downloaded again on every launch.
///
/// Only content-addressed relay media (`/media/<sha256>...`) is stored: those
/// bytes never change for a URL, so an entry never goes stale and a changed
/// group photo is a new URL, not a changed file. Other URLs (third-party
/// avatars) can change behind the same address and are not stored.
///
/// Entries are keyed by `scope` plus URL. Relay media is access-controlled
/// per reader (a private group's photo or attachment is readable only by its
/// members), so the scope is the relay and signed-in identity that fetched
/// the bytes, and another account on this device never reads them from disk.
///
/// The total size is kept near [maxBytes] by deleting the least recently
/// used files after writes. Eviction runs in the background and never delays
/// a load. Every failure is treated as a miss: the network stays the source
/// of truth.
class MediaDiskCache {
  MediaDiskCache(this._directory, {this.maxBytes = mediaDiskCacheMaxBytes});

  final Future<Directory> _directory;

  /// Size budget in bytes.
  final int maxBytes;

  Future<void>? _eviction;
  bool _evictAgain = false;

  /// Whether [url] is content-addressed relay media worth storing.
  static bool isCacheable(String url) {
    final uri = Uri.tryParse(url);
    return uri != null &&
        (uri.scheme == 'https' || uri.scheme == 'http') &&
        !uri.hasQuery &&
        _contentAddressedPath.hasMatch(uri.path);
  }

  Future<File> _fileFor(String scope, String url) async {
    final name = sha256.convert(utf8.encode('$scope\n$url')).toString();
    return File('${(await _directory).path}/$name');
  }

  /// The stored bytes for [url] in [scope], or null on a miss.
  Future<Uint8List?> read(String scope, String url) async {
    if (!isCacheable(url)) return null;
    try {
      final file = await _fileFor(scope, url);
      final bytes = await file.readAsBytes();
      if (bytes.isEmpty) return null;
      // Mark it recently used so eviction keeps it.
      unawaited(file.setLastModified(DateTime.now()).catchError((_) {}));
      return bytes;
    } catch (_) {
      return null;
    }
  }

  /// Deletes the entry for [url] in [scope], if any.
  Future<void> remove(String scope, String url) async {
    if (!isCacheable(url)) return;
    try {
      await (await _fileFor(scope, url)).delete();
    } catch (_) {
      // Nothing stored.
    }
  }

  /// Stores [bytes] for [url] in [scope] and schedules eviction.
  Future<void> write(String scope, String url, Uint8List bytes) async {
    if (!isCacheable(url) || bytes.isEmpty) return;
    try {
      final file = await _fileFor(scope, url);
      await file.parent.create(recursive: true);
      // Write beside the entry and rename, so a reader never sees half a file.
      final partial = File('${file.path}.part');
      await partial.writeAsBytes(bytes, flush: true);
      await partial.rename(file.path);
    } catch (error) {
      debugPrint('[MediaDiskCache] write failed: $error');
      return;
    }
    scheduleEviction();
  }

  /// Starts an eviction pass, or queues one more if a pass is running.
  void scheduleEviction() {
    if (_eviction != null) {
      _evictAgain = true;
      return;
    }
    _eviction = _evict().whenComplete(() {
      _eviction = null;
      if (_evictAgain) {
        _evictAgain = false;
        scheduleEviction();
      }
    });
  }

  /// Completes when no eviction pass is running.
  @visibleForTesting
  Future<void> get evictionIdle async {
    while (_eviction != null) {
      await _eviction;
    }
  }

  Future<void> _evict() async {
    try {
      final directory = await _directory;
      if (!await directory.exists()) return;
      final entries = <(File, int, DateTime)>[];
      var total = 0;
      await for (final entity in directory.list(followLinks: false)) {
        if (entity is! File || entity.path.endsWith('.part')) continue;
        final stat = await entity.stat();
        entries.add((entity, stat.size, stat.modified));
        total += stat.size;
      }
      if (total <= maxBytes) return;
      entries.sort((a, b) => a.$3.compareTo(b.$3));
      for (final (file, size, _) in entries) {
        if (total <= maxBytes) break;
        try {
          await file.delete();
          total -= size;
        } catch (_) {
          // Already gone or busy; the next pass retries.
        }
      }
    } catch (error) {
      debugPrint('[MediaDiskCache] eviction failed: $error');
    }
  }
}

/// The app's media disk cache, under the platform cache directory. Null in
/// widget tests, which have no platform directories; tests that cover the
/// cache build their own [MediaDiskCache].
final mediaDiskCacheProvider = Provider<MediaDiskCache?>((ref) {
  if (!kIsWeb && Platform.environment.containsKey('FLUTTER_TEST')) {
    return null;
  }
  return MediaDiskCache(
    getApplicationCacheDirectory().then(
      (base) => Directory('${base.path}/kuzz-media-v1'),
    ),
  );
});
