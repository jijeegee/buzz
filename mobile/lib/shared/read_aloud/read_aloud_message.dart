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

/// Width of the speaker control's tap target.
const readAloudButtonWidth = 36.0;

/// Height of the speaker control, matching one line of message body copy.
const readAloudButtonHeight = 20.0;

/// Speaker button and spoken-text body wrapper for one message row.
///
/// The row places [button] outside the message body column so read-aloud
/// controls never narrow the message content or add height while idle.
class ReadAloudSlots {
  const ReadAloudSlots({required this.button, required this.wrapBody});

  /// Compact play/stop control for an empty slot in the message row.
  final Widget button;

  /// Wraps the message body with highlighting and, while speaking, status.
  final Widget Function(Widget body) wrapBody;
}

/// Opt-in message controls, shared by channel and thread timelines.
///
/// [builder] receives null slots while read-aloud is disabled.
class ReadAloudMessage extends HookConsumerWidget {
  const ReadAloudMessage({
    super.key,
    required this.messageId,
    required this.content,
    required this.builder,
  });
  final String messageId;
  final String content;
  final Widget Function(BuildContext context, ReadAloudSlots? slots) builder;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final enabled = ref.watch(readAloudEnabledProvider);
    if (!enabled) return builder(context, null);
    return _EnabledReadAloudMessage(
      messageId: messageId,
      content: content,
      builder: builder,
    );
  }
}

/// Places [child] inside [slots]' body wrapper when read-aloud is enabled.
class ReadAloudBody extends StatelessWidget {
  const ReadAloudBody({super.key, required this.slots, required this.child});
  final ReadAloudSlots? slots;
  final Widget child;

  @override
  Widget build(BuildContext context) => slots?.wrapBody(child) ?? child;
}

class _EnabledReadAloudMessage extends HookConsumerWidget {
  const _EnabledReadAloudMessage({
    required this.messageId,
    required this.content,
    required this.builder,
  });
  final String messageId;
  final String content;
  final Widget Function(BuildContext context, ReadAloudSlots? slots) builder;

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
      final language = RegExp(r'[ᄀ-ᇿ㄰-㆏가-힯]').hasMatch(text)
          ? 'ko'
          : RegExp(r'[a-zA-Z]').hasMatch(text)
          ? 'en'
          : Localizations.localeOf(context).languageCode;
      unawaited(controller.play(owner, text, language));
    }

    final paused = session?.phase == ReadAloudPhase.paused;
    final button = _ReadAloudIconButton(
      key: ValueKey('read-aloud-play-$messageId'),
      tooltip: active ? '읽어주기 중지' : '메시지 읽어주기',
      icon: active ? LucideIcons.square : LucideIcons.volume2,
      onPressed: active ? () => unawaited(controller.stop()) : play,
    );

    Widget wrapBody(Widget body) {
      return DecoratedBox(
        key: ValueKey('read-aloud-message-$messageId'),
        decoration: BoxDecoration(
          border: active
              ? Border(
                  left: BorderSide(width: 2, color: context.colors.primary),
                )
              : null,
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
              child: body,
            ),
            if (active)
              Padding(
                padding: const EdgeInsets.only(top: Grid.quarter),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
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
                      ),
                    ),
                    _ReadAloudIconButton(
                      key: ValueKey('read-aloud-pause-$messageId'),
                      tooltip: paused ? '이어 읽기' : '일시정지',
                      icon: paused ? LucideIcons.play : LucideIcons.pause,
                      onPressed: () => unawaited(
                        paused ? controller.resume() : controller.pause(),
                      ),
                    ),
                  ],
                ),
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

    return builder(context, ReadAloudSlots(button: button, wrapBody: wrapBody));
  }
}

class _ReadAloudIconButton extends StatelessWidget {
  const _ReadAloudIconButton({
    super.key,
    required this.tooltip,
    required this.icon,
    required this.onPressed,
  });
  final String tooltip;
  final IconData icon;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: InkResponse(
        onTap: onPressed,
        radius: readAloudButtonWidth / 2,
        child: SizedBox(
          width: readAloudButtonWidth,
          height: readAloudButtonHeight,
          child: Icon(icon, size: 18, color: context.colors.onSurfaceVariant),
        ),
      ),
    );
  }
}
