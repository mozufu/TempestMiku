part of 'main.dart';

// ─── Design tokens (from inm design system) ──────────────────────────────────
// Light: warm stone canvas. Hex values from colors_and_type.css comments.
// Dark: plum black canvas.

class _Tok {
  final Color bg, surface, raised, border, text, muted;
  final Color accent, accentSoft, cool, onAccent;
  final Color success, warning, danger, focus;
  const _Tok({
    required this.bg,
    required this.surface,
    required this.raised,
    required this.border,
    required this.text,
    required this.muted,
    required this.accent,
    required this.accentSoft,
    required this.cool,
    required this.onAccent,
    required this.success,
    required this.warning,
    required this.danger,
    required this.focus,
  });

  static const light = _Tok(
    bg: Color(0xFFF1EDE8),
    surface: Color(0xFFFFF8F2),
    raised: Color(0xFFFFFFFF),
    border: Color(0xFFC9BDB5),
    text: Color(0xFF251D27),
    muted: Color(0xFF5F565D),
    accent: Color(0xFF8E514B),
    accentSoft: Color(0xFFB9786E),
    cool: Color(0xFF466B61),
    onAccent: Color(0xFFFFF8F2),
    success: Color(0xFF2F7D55),
    warning: Color(0xFFB76D2F),
    danger: Color(0xFFA43F39),
    focus: Color(0xFF2F6F9F),
  );

  static const dark = _Tok(
    bg: Color(0xFF302735),
    surface: Color(0xFF3B3140),
    raised: Color(0xFF493D4F),
    border: Color(0xFF746478),
    text: Color(0xFFF8F1EA),
    muted: Color(0xFFD1C6C8),
    accent: Color(0xFFE0A193),
    accentSoft: Color(0xFFCD8D82),
    cool: Color(0xFFA9C8BC),
    onAccent: Color(0xFF251D27),
    success: Color(0xFF78C091),
    warning: Color(0xFFE0AA62),
    danger: Color(0xFFE17765),
    focus: Color(0xFFC6B4FF),
  );
}

// Mode-temperature accent overrides. These are fixed OKLCH values from the design.
Color _modeAccent(String temp, _Tok tok) {
  switch (temp) {
    case 'hot':
      return tok.danger;
    case 'soft':
      return const Color(0xFFAA7860); // oklch(64% 0.058 28) — warm terracotta
    case 'warm':
      return tok.accentSoft;
    case 'cool':
      return tok.cool;
    default:
      return tok.accentSoft;
  }
}

// Pick white or dark text for contrast on a given background.
Color _textOn(Color bg) {
  final lum = (0.299 * bg.red + 0.587 * bg.green + 0.114 * bg.blue) / 255;
  return lum > 0.55 ? const Color(0xFF2F2730) : Colors.white;
}
