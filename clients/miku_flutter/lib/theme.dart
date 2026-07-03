part of 'main.dart';

// ─── Design tokens (from inm design system) ──────────────────────────────────
// Light: warm stone canvas. Hex values from colors_and_type.css comments.
// Dark: plum black canvas.

class _Tok {
  final Color bg, surface, raised, border, text, muted;
  final Color accent, accentSoft, cool, onAccent;
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
  });

  static const light = _Tok(
    bg: Color(0xFFC9BFB6),
    surface: Color(0xFFDAD2CB),
    raised: Color(0xFFE8E1DB),
    border: Color(0xFFAFA8A3),
    text: Color(0xFF2F2730),
    muted: Color(0xFF625B5E),
    accent: Color(0xFF7E4F49),
    accentSoft: Color(0xFFA1736B),
    cool: Color(0xFF79837F),
    onAccent: Color(0xFFE8E1DB),
  );

  static const dark = _Tok(
    bg: Color(0xFF3E343F),
    surface: Color(0xFF493E4A),
    raised: Color(0xFF584B59),
    border: Color(0xFF6B6069),
    text: Color(0xFFE8E1DB),
    muted: Color(0xFFB6AFAD),
    accent: Color(0xFFD7A095),
    accentSoft: Color(0xFFC58E82),
    cool: Color(0xFFAAB4B0),
    onAccent: Color(0xFF3E343F),
  );
}

// Mode-temperature accent overrides. These are fixed OKLCH values from the design.
Color _modeAccent(String temp, _Tok tok) {
  switch (temp) {
    case 'hot':
      return const Color(
          0xFFB84A30); // oklch(58% 0.094 33) — saturated rust orange
    case 'soft':
      return const Color(0xFFAA7860); // oklch(64% 0.058 28) — warm terracotta
    case 'warm':
      return const Color(0xFFA1736B); // clay-500
    case 'cool':
      return tok.cool;
    default:
      return const Color(0xFFA1736B);
  }
}

// Pick white or dark text for contrast on a given background.
Color _textOn(Color bg) {
  final lum = (0.299 * bg.red + 0.587 * bg.green + 0.114 * bg.blue) / 255;
  return lum > 0.55 ? const Color(0xFF2F2730) : Colors.white;
}
