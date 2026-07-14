// ignore_for_file: avoid_web_libraries_in_flutter, deprecated_member_use

import 'dart:html' as html;
import 'dart:ui_web' as ui_web;

import 'package:flutter/material.dart';

class RaTeXFormula extends StatefulWidget {
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
  State<RaTeXFormula> createState() => _RaTeXFormulaState();
}

class _RaTeXFormulaState extends State<RaTeXFormula> {
  static var _nextViewId = 0;

  late final String _viewType = 'ratex-formula-${_nextViewId++}';
  html.Element? _element;

  @override
  void initState() {
    super.initState();
    ui_web.platformViewRegistry.registerViewFactory(_viewType, (int viewId) {
      final element =
          html.Element.tag('miku-ratex-formula')
            ..style.display = 'block'
            ..style.width = '100%'
            ..style.height = '100%'
            ..style.overflowX = 'auto'
            ..style.overflowY = 'hidden';
      _element = element;
      _applyAttributes();
      return element;
    });
  }

  @override
  void didUpdateWidget(covariant RaTeXFormula oldWidget) {
    super.didUpdateWidget(oldWidget);
    _applyAttributes();
  }

  void _applyAttributes() {
    final element = _element;
    if (element == null) return;
    element
      ..setAttribute('latex', widget.latex)
      ..setAttribute('font-size', widget.fontSize.toStringAsFixed(1))
      ..setAttribute('padding', widget.display ? '8' : '1')
      ..setAttribute('background-color', 'transparent')
      ..setAttribute('color', _hexColor(widget.color))
      ..setAttribute('display', widget.display ? 'block' : 'inline')
      ..setAttribute('aria-label', widget.latex)
      ..setAttribute('title', widget.latex);
  }

  @override
  Widget build(BuildContext context) {
    final height =
        widget.display
            ? _displayHeight(widget.latex, widget.fontSize)
            : (widget.fontSize * 1.65).clamp(18.0, 32.0);
    return SizedBox(
      height: height.toDouble(),
      width: widget.display ? double.infinity : _inlineWidth(widget.latex),
      child: HtmlElementView(viewType: _viewType),
    );
  }
}

String _hexColor(Color color) {
  final value = color.toARGB32() & 0x00ffffff;
  return '#${value.toRadixString(16).padLeft(6, '0')}';
}

double _inlineWidth(String latex) {
  final compact = latex.replaceAll(RegExp(r'\s+'), ' ');
  return (compact.length * 8.5 + 18).clamp(28.0, 260.0).toDouble();
}

double _displayHeight(String latex, double fontSize) {
  final hasTallOperator = RegExp(
    r'\\(?:sum|prod|int)|\\frac|\\boxed',
  ).hasMatch(latex);
  final hasLineBreak = latex.contains('\n') || latex.contains(r'\\');
  final multiplier =
      hasLineBreak
          ? 4.9
          : hasTallOperator
          ? 4.35
          : 3.65;
  return (fontSize * multiplier).clamp(64.0, 104.0).toDouble();
}
