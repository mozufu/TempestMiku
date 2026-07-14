part of 'main.dart';

/// Abstract cat ears cut by a lightning bolt: Miku identity without character
/// art, and legible at the same sizes as a Material icon.
class MikuStormCatMark extends StatelessWidget {
  const MikuStormCatMark({
    super.key,
    this.size,
    this.color,
    this.boltColor,
    this.semanticLabel,
  });

  final double? size;
  final Color? color;
  final Color? boltColor;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final iconTheme = IconTheme.of(context);
    final tok = MikuTokens.of(context);
    final resolvedSize = size ?? iconTheme.size ?? 24;
    final mark = CustomPaint(
      size: Size.square(resolvedSize),
      painter: _StormCatPainter(
        color: color ?? iconTheme.color ?? tok.accent,
        boltColor: boltColor ?? tok.coral,
      ),
    );
    if (semanticLabel == null) return ExcludeSemantics(child: mark);
    return Semantics(
      image: true,
      label: semanticLabel,
      child: ExcludeSemantics(child: mark),
    );
  }
}

class MikuBrandBadge extends StatelessWidget {
  const MikuBrandBadge({
    super.key,
    this.size = 40,
    this.semanticLabel = 'Tempest Miku',
  });

  final double size;
  final String semanticLabel;

  @override
  Widget build(BuildContext context) {
    final tok = MikuTokens.of(context);
    return Semantics(
      image: true,
      label: semanticLabel,
      child: ExcludeSemantics(
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: tok.text,
            borderRadius: BorderRadius.circular(size * 0.3),
            border: Border.all(color: tok.glassBorder),
            boxShadow: [
              BoxShadow(
                color: tok.glow,
                blurRadius: size * 0.45,
                offset: Offset(0, size * 0.12),
              ),
            ],
          ),
          child: Padding(
            padding: EdgeInsets.all(size * 0.19),
            child: MikuStormCatMark(
              size: size * 0.62,
              color: const Color(0xFF39C5BB),
              boltColor: const Color(0xFFFF7B70),
            ),
          ),
        ),
      ),
    );
  }
}

class _StormCatPainter extends CustomPainter {
  const _StormCatPainter({required this.color, required this.boltColor});

  final Color color;
  final Color boltColor;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.save();
    canvas.scale(size.width / 24, size.height / 24);

    final silhouette =
        Path()
          ..moveTo(3.4, 9.6)
          ..lineTo(5.3, 3.4)
          ..lineTo(10.0, 6.9)
          ..cubicTo(10.7, 6.7, 11.3, 6.6, 12.0, 6.6)
          ..cubicTo(12.7, 6.6, 13.3, 6.7, 14.0, 6.9)
          ..lineTo(18.7, 3.4)
          ..lineTo(20.6, 9.6)
          ..cubicTo(21.6, 10.9, 22.0, 12.4, 22.0, 14.1)
          ..cubicTo(22.0, 18.7, 18.5, 21.3, 12.0, 21.3)
          ..cubicTo(5.5, 21.3, 2.0, 18.7, 2.0, 14.1)
          ..cubicTo(2.0, 12.4, 2.4, 10.9, 3.4, 9.6)
          ..close();
    canvas.drawPath(silhouette, Paint()..color = color);

    final bolt =
        Path()
          ..moveTo(12.8, 8.1)
          ..lineTo(8.8, 14.0)
          ..lineTo(11.9, 14.0)
          ..lineTo(10.7, 19.0)
          ..lineTo(16.2, 12.2)
          ..lineTo(13.2, 12.2)
          ..lineTo(15.1, 8.1)
          ..close();
    canvas.drawPath(bolt, Paint()..color = boltColor);
    canvas.restore();
  }

  @override
  bool shouldRepaint(covariant _StormCatPainter oldDelegate) {
    return oldDelegate.color != color || oldDelegate.boltColor != boltColor;
  }
}
