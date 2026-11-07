import 'dart:math' as math;
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';

import 'dot_shapes.dart';

/// Loading animations drawn in dots, as on kuzz.app and in the desktop app.
///
/// A scene is a fixed number of square dots that travel between shapes: the
/// K, people and robots. The engine mirrors `dotMorph.ts` in the desktop app.

typedef DotSceneData = ({
  double width,
  double height,
  int count,
  double dotSize,
  double dustSpread,
  Map<String, List<int>> shapes,
});

/// One timeline step: shape key, transition, hold.
typedef DotStep = (String key, Duration transition, Duration hold);

const dustShape = 'dust';

// Share of dots drawn in the accent color, as on kuzz.app.
const _accentShare = 0.05;
// How late (as a share of the transition) the slowest dot starts moving.
const _maxLag = 0.45;
// Accent dots use the kuzz.app yellows, brighter on dark backgrounds.
const _accentLight = Color(0xFFF0A500);
const _accentDark = Color(0xFFFFD84D);

double _hash(int i, int k) {
  final v = math.sin(i * 12.9898 + k * 78.233) * 43758.5453;
  return v - v.floorToDouble();
}

double _smooth(double t) => t * t * (3 - 2 * t);

bool _isAccentDot(int i) => _hash(i, 9) <= _accentShare;

/// Shapes of one scene, in scene units, plus scattered "dust".
class DotScene {
  final double width;
  final double height;
  final int count;
  final double dotSize;
  final Map<String, Float32List> shapes;

  DotScene._(this.width, this.height, this.count, this.dotSize, this.shapes);

  factory DotScene.fromData(DotSceneData data) {
    final shapes = <String, Float32List>{
      for (final entry in data.shapes.entries)
        entry.key: Float32List.fromList([for (final v in entry.value) v / 10]),
    };
    final dust = Float32List(data.count * 2);
    for (var i = 0; i < data.count; i++) {
      dust[2 * i] =
          data.width / 2 +
          (_hash(i, 1) - 0.5) * data.width * 1.15 * data.dustSpread;
      dust[2 * i + 1] =
          data.height / 2 +
          (_hash(i, 2) - 0.5) * data.height * 1.15 * data.dustSpread;
    }
    shapes[dustShape] = dust;
    return DotScene._(
      data.width,
      data.height,
      data.count,
      data.dotSize,
      shapes,
    );
  }

  static final refresh = DotScene.fromData(refreshDotScene);
  static final huddle = DotScene.fromData(huddleDotScene);

  Float32List shape(String key) {
    final shape = shapes[key];
    if (shape == null) throw ArgumentError.value(key, 'key', 'Unknown shape');
    return shape;
  }
}

/// Moves a scene's dots between shapes.
class DotMorph extends ChangeNotifier {
  final DotScene scene;
  final Float32List positions;
  Float32List _from;
  Float32List _to;
  double _progress = 1;
  double _durationMs = 1;

  DotMorph(this.scene, String initial)
    : positions = Float32List.fromList(scene.shape(initial)),
      _from = scene.shape(initial),
      _to = scene.shape(initial);

  bool get settled => _progress >= 1;

  /// Starts moving every dot from where it is now to [key].
  void go(String key, Duration duration) {
    _from = Float32List.fromList(positions);
    _to = scene.shape(key);
    final ms = duration.inMicroseconds / 1000;
    _progress = ms > 0 ? 0 : 1;
    _durationMs = math.max(1, ms);
    if (_progress >= 1) _tick(0);
  }

  /// Places the dots part of the way from [from] to [to], following a finger.
  void blend(String from, String to, double t) {
    final a = scene.shape(from);
    final b = scene.shape(to);
    for (var i = 0; i < scene.count; i++) {
      final lag = _hash(i, 5) * _maxLag * 0.8;
      final u = _smooth(((t - lag) / (1 - lag)).clamp(0.0, 1.0));
      positions[2 * i] = a[2 * i] + (b[2 * i] - a[2 * i]) * u;
      positions[2 * i + 1] = a[2 * i + 1] + (b[2 * i + 1] - a[2 * i + 1]) * u;
    }
    _from = _to = b;
    _progress = 1;
    notifyListeners();
  }

  void tick(Duration elapsed) => _tick(elapsed.inMicroseconds / 1000);

  void _tick(double dtMs) {
    _progress = math.min(1, _progress + dtMs / _durationMs);
    for (var i = 0; i < scene.count; i++) {
      final lag = _hash(i, 5) * _maxLag;
      final u = _smooth(((_progress - lag) / (1 - lag)).clamp(0.0, 1.0));
      // Each dot bows sideways on its way, so shapes swirl instead of sliding.
      final bow =
          math.sin(u * math.pi) * scene.height * 0.16 * (_hash(i, 6) - 0.5);
      positions[2 * i] = _from[2 * i] + (_to[2 * i] - _from[2 * i]) * u + bow;
      positions[2 * i + 1] =
          _from[2 * i + 1] +
          (_to[2 * i + 1] - _from[2 * i + 1]) * u -
          bow.abs() * 0.6;
    }
    notifyListeners();
  }
}

/// Plays steps in order, forever. The first step appears without a transition.
class DotTimeline {
  final DotMorph morph;
  final List<DotStep> steps;
  int _index;
  double _waitMs;

  DotTimeline(this.morph, this.steps)
    : _index = steps.length > 1 ? 1 : 0,
      _waitMs = _ms(steps.first.$2) + _ms(steps.first.$3);

  static double _ms(Duration d) => d.inMicroseconds / 1000;

  void tick(Duration elapsed) {
    _waitMs -= _ms(elapsed);
    if (_waitMs <= 0 && steps.length > 1) {
      final (key, transition, hold) = steps[_index];
      morph.go(key, transition);
      _waitMs = _ms(transition) + _ms(hold);
      _index = (_index + 1) % steps.length;
    }
    morph.tick(elapsed);
  }
}

/// Calls [onFrame] with the time since the previous frame while [enabled].
void useDotFrames(bool enabled, void Function(Duration elapsed) onFrame) {
  final vsync = useSingleTickerProvider();
  final callback = useRef(onFrame)..value = onFrame;
  useEffect(() {
    if (!enabled) return null;
    var last = Duration.zero;
    final ticker = vsync.createTicker((now) {
      // Cap a frame so a stalled app does not skip a whole transition.
      final step = now - last;
      last = now;
      callback.value(
        step > const Duration(milliseconds: 50)
            ? const Duration(milliseconds: 50)
            : step,
      );
    });
    ticker.start();
    return ticker.dispose;
  }, [vsync, enabled]);
}

/// Paints a [DotMorph] in [color], with a few accent dots.
class DotMorphView extends StatelessWidget {
  final DotMorph morph;
  final Color color;

  const DotMorphView({required this.morph, required this.color, super.key});

  @override
  Widget build(BuildContext context) {
    final accent = Theme.of(context).brightness == Brightness.dark
        ? _accentDark
        : _accentLight;
    return CustomPaint(
      size: Size(morph.scene.width, morph.scene.height),
      painter: _DotMorphPainter(morph, color, accent),
    );
  }
}

class _DotMorphPainter extends CustomPainter {
  final DotMorph morph;
  final Color color;
  final Color accent;

  _DotMorphPainter(this.morph, this.color, this.accent) : super(repaint: morph);

  @override
  void paint(Canvas canvas, Size size) {
    final scene = morph.scene;
    canvas.scale(size.width / scene.width, size.height / scene.height);
    final half = scene.dotSize / 2;
    final base = Path();
    final highlight = Path();
    for (var i = 0; i < scene.count; i++) {
      final x = morph.positions[2 * i];
      final y = morph.positions[2 * i + 1];
      (_isAccentDot(i) ? highlight : base).addRect(
        Rect.fromLTWH(x - half, y - half, scene.dotSize, scene.dotSize),
      );
    }
    canvas
      ..drawPath(base, Paint()..color = color)
      ..drawPath(highlight, Paint()..color = accent);
  }

  @override
  bool shouldRepaint(_DotMorphPainter oldDelegate) =>
      morph != oldDelegate.morph ||
      color != oldDelegate.color ||
      accent != oldDelegate.accent;
}

/// A looping dot animation. With reduced motion it shows the last step still.
class DotMorphLoader extends HookWidget {
  final DotScene scene;
  final List<DotStep> steps;
  final Color color;

  const DotMorphLoader({
    required this.scene,
    required this.steps,
    required this.color,
    super.key,
  });

  /// My side crosses to the other side and back, then both stay.
  static const huddleSteps = <DotStep>[
    ('l', Duration(milliseconds: 500), Duration(milliseconds: 300)),
    ('r', Duration(milliseconds: 800), Duration(milliseconds: 300)),
    ('l', Duration(milliseconds: 800), Duration(milliseconds: 300)),
    ('r', Duration(milliseconds: 800), Duration(milliseconds: 300)),
    ('b', Duration(milliseconds: 500), Duration(milliseconds: 1600)),
  ];

  @override
  Widget build(BuildContext context) {
    final still = MediaQuery.disableAnimationsOf(context);
    final morph = useMemoized(
      () => DotMorph(scene, still ? steps.last.$1 : steps.first.$1),
      [scene, still],
    );
    useEffect(() => morph.dispose, [morph]);
    final timeline = useMemoized(() => DotTimeline(morph, steps), [morph]);
    useDotFrames(!still, timeline.tick);
    return DotMorphView(morph: morph, color: color);
  }
}
