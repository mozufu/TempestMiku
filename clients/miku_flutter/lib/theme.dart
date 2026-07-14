part of 'main.dart';

// Material 3 foundations for Tempest Miku. The public extension is the source
// of truth for new UI; `_Tok` remains as a small compatibility layer for the
// existing feature surfaces while they move to context-owned theming.

@immutable
class MikuTokens extends ThemeExtension<MikuTokens> {
  const MikuTokens({
    required this.bg,
    required this.surface,
    required this.raised,
    required this.border,
    required this.text,
    required this.muted,
    required this.accent,
    required this.accentSoft,
    required this.cool,
    required this.coral,
    required this.onAccent,
    required this.success,
    required this.warning,
    required this.danger,
    required this.focus,
    required this.glass,
    required this.glassBorder,
    required this.glow,
  });

  final Color bg;
  final Color surface;
  final Color raised;
  final Color border;
  final Color text;
  final Color muted;
  final Color accent;
  final Color accentSoft;
  final Color cool;
  final Color coral;
  final Color onAccent;
  final Color success;
  final Color warning;
  final Color danger;
  final Color focus;
  final Color glass;
  final Color glassBorder;
  final Color glow;

  static MikuTokens get light => _Tok.light;
  static MikuTokens get dark => _Tok.dark;

  static MikuTokens of(BuildContext context) {
    final theme = Theme.of(context);
    return theme.extension<MikuTokens>() ??
        (theme.brightness == Brightness.dark ? _Tok.dark : _Tok.light);
  }

  @override
  MikuTokens copyWith({
    Color? bg,
    Color? surface,
    Color? raised,
    Color? border,
    Color? text,
    Color? muted,
    Color? accent,
    Color? accentSoft,
    Color? cool,
    Color? coral,
    Color? onAccent,
    Color? success,
    Color? warning,
    Color? danger,
    Color? focus,
    Color? glass,
    Color? glassBorder,
    Color? glow,
  }) {
    return MikuTokens(
      bg: bg ?? this.bg,
      surface: surface ?? this.surface,
      raised: raised ?? this.raised,
      border: border ?? this.border,
      text: text ?? this.text,
      muted: muted ?? this.muted,
      accent: accent ?? this.accent,
      accentSoft: accentSoft ?? this.accentSoft,
      cool: cool ?? this.cool,
      coral: coral ?? this.coral,
      onAccent: onAccent ?? this.onAccent,
      success: success ?? this.success,
      warning: warning ?? this.warning,
      danger: danger ?? this.danger,
      focus: focus ?? this.focus,
      glass: glass ?? this.glass,
      glassBorder: glassBorder ?? this.glassBorder,
      glow: glow ?? this.glow,
    );
  }

  @override
  MikuTokens lerp(covariant MikuTokens? other, double t) {
    if (other == null) return this;
    return MikuTokens(
      bg: Color.lerp(bg, other.bg, t)!,
      surface: Color.lerp(surface, other.surface, t)!,
      raised: Color.lerp(raised, other.raised, t)!,
      border: Color.lerp(border, other.border, t)!,
      text: Color.lerp(text, other.text, t)!,
      muted: Color.lerp(muted, other.muted, t)!,
      accent: Color.lerp(accent, other.accent, t)!,
      accentSoft: Color.lerp(accentSoft, other.accentSoft, t)!,
      cool: Color.lerp(cool, other.cool, t)!,
      coral: Color.lerp(coral, other.coral, t)!,
      onAccent: Color.lerp(onAccent, other.onAccent, t)!,
      success: Color.lerp(success, other.success, t)!,
      warning: Color.lerp(warning, other.warning, t)!,
      danger: Color.lerp(danger, other.danger, t)!,
      focus: Color.lerp(focus, other.focus, t)!,
      glass: Color.lerp(glass, other.glass, t)!,
      glassBorder: Color.lerp(glassBorder, other.glassBorder, t)!,
      glow: Color.lerp(glow, other.glow, t)!,
    );
  }
}

class _Tok extends MikuTokens {
  const _Tok({
    required super.bg,
    required super.surface,
    required super.raised,
    required super.border,
    required super.text,
    required super.muted,
    required super.accent,
    required super.accentSoft,
    required super.cool,
    required super.coral,
    required super.onAccent,
    required super.success,
    required super.warning,
    required super.danger,
    required super.focus,
    required super.glass,
    required super.glassBorder,
    required super.glow,
  });

  static const light = _Tok(
    bg: Color(0xFFF2F8F7),
    surface: Color(0xFFFAFDFC),
    raised: Color(0xFFFFFFFF),
    border: Color(0xFFB9CECA),
    text: Color(0xFF102A2D),
    muted: Color(0xFF4D6263),
    accent: Color(0xFF006B66),
    accentSoft: Color(0xFF39C5BB),
    cool: Color(0xFF315E66),
    coral: Color(0xFFB83F37),
    onAccent: Color(0xFFFFFFFF),
    success: Color(0xFF176B4E),
    warning: Color(0xFF8B5700),
    danger: Color(0xFFB3261E),
    focus: Color(0xFF005AC1),
    glass: Color(0xE6FFFFFF),
    glassBorder: Color(0x8FB9CECA),
    glow: Color(0x3339C5BB),
  );

  static const dark = _Tok(
    bg: Color(0xFF071D20),
    surface: Color(0xFF0E292C),
    raised: Color(0xFF173437),
    border: Color(0xFF496467),
    text: Color(0xFFF0FBF9),
    muted: Color(0xFFB2C9C6),
    accent: Color(0xFF73DDD3),
    accentSoft: Color(0xFF39C5BB),
    cool: Color(0xFFA1CED5),
    coral: Color(0xFFFFB4AA),
    onAccent: Color(0xFF003735),
    success: Color(0xFF7BDDAF),
    warning: Color(0xFFFFB95F),
    danger: Color(0xFFFFB4AB),
    focus: Color(0xFFAFC6FF),
    glass: Color(0xE60E292C),
    glassBorder: Color(0xA3496467),
    glow: Color(0x4039C5BB),
  );
}

abstract final class MikuTheme {
  static const _fontFamily = 'MikuCjkUi';
  static const _fontFallbacks = <String>[
    '.SF Pro Text',
    'Segoe UI',
    'Roboto',
    'PingFang TC',
    'PingFang SC',
    'Noto Sans CJK TC',
    'Noto Sans CJK SC',
    'Noto Sans TC',
    'Noto Sans SC',
    'Microsoft JhengHei',
    'Microsoft YaHei',
    'Arial',
    'sans-serif',
  ];

  static ThemeData get light => _build(Brightness.light, _Tok.light);
  static ThemeData get dark => _build(Brightness.dark, _Tok.dark);

  static ThemeData _build(Brightness brightness, _Tok tok) {
    final isDark = brightness == Brightness.dark;
    final scheme = ColorScheme.fromSeed(
      seedColor: const Color(0xFF39C5BB),
      brightness: brightness,
    ).copyWith(
      primary: tok.accent,
      onPrimary: tok.onAccent,
      primaryContainer:
          isDark ? const Color(0xFF00504C) : const Color(0xFFB9F2EC),
      onPrimaryContainer:
          isDark ? const Color(0xFFB9F2EC) : const Color(0xFF00201E),
      secondary: tok.coral,
      onSecondary: isDark ? const Color(0xFF690005) : const Color(0xFFFFFFFF),
      secondaryContainer:
          isDark ? const Color(0xFF8F1418) : const Color(0xFFFFDAD5),
      onSecondaryContainer:
          isDark ? const Color(0xFFFFDAD5) : const Color(0xFF410002),
      surface: tok.surface,
      onSurface: tok.text,
      outline: tok.border,
      error: tok.danger,
      onError: isDark ? const Color(0xFF690005) : const Color(0xFFFFFFFF),
    );

    final base = ThemeData(
      useMaterial3: true,
      brightness: brightness,
      colorScheme: scheme,
      scaffoldBackgroundColor: tok.bg,
      canvasColor: tok.bg,
      cardColor: tok.raised,
      dividerColor: tok.border,
      disabledColor: tok.muted.withValues(alpha: 0.48),
      focusColor: tok.focus.withValues(alpha: 0.18),
      hoverColor: tok.accent.withValues(alpha: 0.08),
      highlightColor: tok.accent.withValues(alpha: 0.12),
      splashFactory: InkSparkle.splashFactory,
      fontFamily: _fontFamily,
      fontFamilyFallback: _fontFallbacks,
      materialTapTargetSize: MaterialTapTargetSize.padded,
      visualDensity: VisualDensity.standard,
      extensions: <ThemeExtension<dynamic>>[tok],
    );

    final textTheme = base.textTheme
        .apply(bodyColor: tok.text, displayColor: tok.text)
        .copyWith(
          headlineLarge: base.textTheme.headlineLarge?.copyWith(
            fontWeight: FontWeight.w700,
            letterSpacing: -0.8,
          ),
          headlineMedium: base.textTheme.headlineMedium?.copyWith(
            fontWeight: FontWeight.w700,
            letterSpacing: -0.5,
          ),
          titleLarge: base.textTheme.titleLarge?.copyWith(
            fontWeight: FontWeight.w700,
            letterSpacing: -0.2,
          ),
          titleMedium: base.textTheme.titleMedium?.copyWith(
            fontWeight: FontWeight.w600,
          ),
          bodyLarge: base.textTheme.bodyLarge?.copyWith(height: 1.5),
          bodyMedium: base.textTheme.bodyMedium?.copyWith(height: 1.48),
          labelLarge: base.textTheme.labelLarge?.copyWith(
            fontWeight: FontWeight.w700,
            letterSpacing: 0.1,
          ),
        );

    final roundedShape = RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(16),
    );
    final inputBorder = OutlineInputBorder(
      borderRadius: BorderRadius.circular(18),
      borderSide: BorderSide(color: tok.border),
    );

    return base.copyWith(
      textTheme: textTheme,
      iconTheme: IconThemeData(color: tok.text, size: 22),
      primaryIconTheme: IconThemeData(color: tok.onAccent, size: 22),
      appBarTheme: AppBarTheme(
        backgroundColor: tok.glass,
        foregroundColor: tok.text,
        surfaceTintColor: Colors.transparent,
        elevation: 0,
        scrolledUnderElevation: 0,
        centerTitle: false,
        systemOverlayStyle:
            isDark ? SystemUiOverlayStyle.light : SystemUiOverlayStyle.dark,
        titleTextStyle: textTheme.titleLarge?.copyWith(color: tok.text),
      ),
      navigationBarTheme: NavigationBarThemeData(
        height: 72,
        backgroundColor: tok.glass,
        indicatorColor: tok.accent.withValues(alpha: isDark ? 0.24 : 0.14),
        surfaceTintColor: Colors.transparent,
        elevation: 0,
      ),
      navigationRailTheme: NavigationRailThemeData(
        backgroundColor: tok.surface,
        indicatorColor: tok.accent.withValues(alpha: isDark ? 0.24 : 0.14),
        selectedIconTheme: IconThemeData(color: tok.accent),
        unselectedIconTheme: IconThemeData(color: tok.muted),
        selectedLabelTextStyle: textTheme.labelMedium?.copyWith(
          color: tok.accent,
          fontWeight: FontWeight.w700,
        ),
        unselectedLabelTextStyle: textTheme.labelMedium?.copyWith(
          color: tok.muted,
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: tok.raised,
        contentPadding: const EdgeInsets.symmetric(
          horizontal: 18,
          vertical: 15,
        ),
        hintStyle: textTheme.bodyLarge?.copyWith(color: tok.muted),
        labelStyle: textTheme.bodyMedium?.copyWith(color: tok.muted),
        border: inputBorder,
        enabledBorder: inputBorder,
        focusedBorder: inputBorder.copyWith(
          borderSide: BorderSide(color: tok.focus, width: 2),
        ),
        errorBorder: inputBorder.copyWith(
          borderSide: BorderSide(color: tok.danger),
        ),
        focusedErrorBorder: inputBorder.copyWith(
          borderSide: BorderSide(color: tok.danger, width: 2),
        ),
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          minimumSize: const Size(48, 48),
          padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
          shape: roundedShape,
          textStyle: textTheme.labelLarge,
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          minimumSize: const Size(48, 48),
          padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
          side: BorderSide(color: tok.border),
          shape: roundedShape,
          textStyle: textTheme.labelLarge,
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(
          minimumSize: const Size(48, 48),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
          shape: roundedShape,
          textStyle: textTheme.labelLarge,
        ),
      ),
      floatingActionButtonTheme: FloatingActionButtonThemeData(
        backgroundColor: tok.accent,
        foregroundColor: tok.onAccent,
        elevation: 2,
        focusElevation: 3,
        hoverElevation: 3,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18)),
      ),
      snackBarTheme: SnackBarThemeData(
        behavior: SnackBarBehavior.floating,
        backgroundColor: isDark ? tok.raised : tok.text,
        contentTextStyle: textTheme.bodyMedium?.copyWith(
          color: isDark ? tok.text : tok.raised,
        ),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      ),
      bottomSheetTheme: BottomSheetThemeData(
        backgroundColor: tok.surface,
        modalBackgroundColor: tok.surface,
        surfaceTintColor: Colors.transparent,
        showDragHandle: false,
        dragHandleColor: tok.border,
        shape: const RoundedRectangleBorder(
          borderRadius: BorderRadius.vertical(top: Radius.circular(28)),
        ),
      ),
      dividerTheme: DividerThemeData(color: tok.border, thickness: 1, space: 1),
    );
  }
}

// Mode-temperature accents remain available for the advanced runtime surfaces.
Color _modeAccent(String temp, _Tok tok) {
  switch (temp) {
    case 'hot':
      return tok.danger;
    case 'soft':
      return tok.coral;
    case 'warm':
      return tok.accentSoft;
    case 'cool':
      return tok.cool;
    default:
      return tok.accentSoft;
  }
}

// Choose a readable ink color for arbitrary activity and mode accents.
Color _textOn(Color bg) {
  return ThemeData.estimateBrightnessForColor(bg) == Brightness.dark
      ? Colors.white
      : const Color(0xFF071D20);
}
