import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../theme/theme.dart';
import 'read_aloud_controller.dart';
import 'read_aloud_preferences.dart';
import 'spoken_text_surface.dart';

/// Stops speech when its conversation is covered or popped.
final readAloudRouteObserver = RouteObserver<ModalRoute<void>>();

class _SpeechRouteAware with RouteAware {
  _SpeechRouteAware(this.stop);
  final VoidCallback stop;
  @override
  void didPushNext() => stop();
  @override
  void didPop() => stop();
}

/// Opt-in message controls, shared by channel and thread timelines.
class ReadAloudMessage extends HookConsumerWidget {
  const ReadAloudMessage({
    super.key,
    required this.messageId,
    required this.content,
    required this.child,
  });
  final String messageId;
  final String content;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final enabled = ref.watch(readAloudEnabledProvider);
    if (!enabled) return child;
    return _EnabledReadAloudMessage(
      messageId: messageId,
      content: content,
      child: child,
    );
  }
}

class _EnabledReadAloudMessage extends HookConsumerWidget {
  const _EnabledReadAloudMessage({
    required this.messageId,
    required this.content,
    required this.child,
  });
  final String messageId;
  final String content;
  final Widget child;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final owner = useMemoized(Object.new, [messageId, content]);
    final surfaceKey = useMemoized(GlobalKey.new);
    final controller = ref.read(readAloudControllerProvider.notifier);
    final session = ref.watch(
      readAloudControllerProvider.select(
        (state) => identical(state.owner, owner) ? state : null,
      ),
    );
    final active = session?.active ?? false;
    useAutomaticKeepAlive(wantKeepAlive: active);
    final route = ModalRoute.of(context);
    final observer = useMemoized(
      () => _SpeechRouteAware(() => unawaited(controller.stopOwner(owner))),
      [owner, controller],
    );
    useEffect(() {
      if (route != null) readAloudRouteObserver.subscribe(observer, route);
      return () {
        readAloudRouteObserver.unsubscribe(observer);
        // Provider writes are deferred out of the widget unmount/build phase.
        scheduleMicrotask(() => controller.stopOwner(owner));
      };
    }, [route, observer]);

    void play() {
      final surface = surfaceKey.currentContext?.findRenderObject();
      if (surface is! SpokenTextRenderBox) return;
      final text = surface.captureText();
      final language =
          RegExp(r'[\u1100-\u11ff\u3130-\u318f\uac00-\ud7af]').hasMatch(text)
          ? 'ko'
          : RegExp(r'[a-zA-Z]').hasMatch(text)
          ? 'en'
          : Localizations.localeOf(context).languageCode;
      unawaited(controller.play(owner, text, language));
    }

    final paused = session?.phase == ReadAloudPhase.paused;
    return DecoratedBox(
      key: ValueKey('read-aloud-message-$messageId'),
      decoration: BoxDecoration(
        border: Border(
          left: BorderSide(
            width: 2,
            color: active ? context.colors.primary : Colors.transparent,
          ),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SpokenTextSurface(
            key: surfaceKey,
            spokenText: active ? session?.text : null,
            start: session?.start ?? 0,
            end: session?.end ?? 0,
            color: context.colors.primary,
            child: child,
          ),
          Row(
            children: [
              Expanded(
                child: active
                    ? Text(
                        session?.phase == ReadAloudPhase.preparing
                            ? '음성 준비 중…'
                            : paused
                            ? (session!.hasProgress
                                  ? '일시정지됨'
                                  : '일시정지 · 이 구간부터 이어읽기')
                            : (session!.hasProgress
                                  ? '읽는 중'
                                  : '읽는 중 · 단어 위치 정보 대기'),
                        style: context.textTheme.labelSmall?.copyWith(
                          color: context.colors.primary,
                        ),
                      )
                    : const SizedBox.shrink(),
              ),
              if (active)
                IconButton(
                  key: ValueKey('read-aloud-pause-$messageId'),
                  tooltip: paused ? '이어 읽기' : '일시정지',
                  onPressed: () => unawaited(
                    paused ? controller.resume() : controller.pause(),
                  ),
                  icon: Icon(
                    paused ? LucideIcons.play : LucideIcons.pause,
                    size: 18,
                  ),
                ),
              IconButton(
                key: ValueKey('read-aloud-play-$messageId'),
                tooltip: active ? '읽어주기 중지' : '메시지 읽어주기',
                onPressed: active ? () => unawaited(controller.stop()) : play,
                icon: Icon(
                  active ? LucideIcons.square : LucideIcons.volume2,
                  size: 18,
                ),
              ),
            ],
          ),
          if (session?.error case final String error)
            Padding(
              padding: const EdgeInsets.only(bottom: Grid.xxs),
              child: Text(
                error,
                style: context.textTheme.bodySmall?.copyWith(
                  color: context.colors.error,
                ),
              ),
            ),
        ],
      ),
    );
  }
}
