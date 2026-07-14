import 'package:flutter/material.dart';

class RaTeXFormula extends StatelessWidget {
  const RaTeXFormula({
    super.key,
    required this.latex,
    required this.fontSize,
    required this.color,
    required this.display,
    this.fallbackStyle,
  });

  final String latex;
  final double fontSize;
  final Color color;
  final bool display;
  final TextStyle? fallbackStyle;

  @override
  Widget build(BuildContext context) {
    return Text(
      latex,
      style:
          fallbackStyle ??
          TextStyle(
            color: color,
            fontSize: fontSize,
            height: display ? 1.45 : 1.2,
            fontFamily: 'monospace',
          ),
    );
  }
}
