import 'package:flutter/material.dart';

/// Single source of truth for TempestMiku's color and spacing tokens, replacing
/// the palette that used to be split between `conversation_app.dart`'s
/// `ColorScheme` overrides and a private class duplicated in `conversation_screen.dart`.
@immutable
class TmTokens extends ThemeExtension<TmTokens> {
  const TmTokens({
    required this.miku,
    required this.muted,
    required this.outline,
    required this.userBubble,
    required this.warm,
    required this.approvalSurface,
    required this.approvalOutline,
    required this.rose,
    required this.roseSoft,
    required this.moss,
    required this.mossSoft,
  });

  const TmTokens.light()
    : miku = const Color(0xff167f78),
      muted = const Color(0xff657378),
      outline = const Color(0xffd9dfdd),
      userBubble = const Color(0xffe4efeb),
      warm = const Color(0xff9a5c18),
      approvalSurface = const Color(0xfffff7ed),
      approvalOutline = const Color(0xffe4c49d),
      rose = const Color(0xffc1503f),
      roseSoft = const Color(0xfffbeae7),
      moss = const Color(0xff3f8368),
      mossSoft = const Color(0xffe7f3ec);

  const TmTokens.dark()
    : miku = const Color(0xff5fd0c5),
      muted = const Color(0xff9aa8ae),
      outline = const Color(0xff28353b),
      userBubble = const Color(0xff1a292f),
      warm = const Color(0xffffc786),
      approvalSurface = const Color(0xff211c18),
      approvalOutline = const Color(0xff5d4934),
      rose = const Color(0xffe2685a),
      roseSoft = const Color(0xff2c1512),
      moss = const Color(0xff7bc4a2),
      mossSoft = const Color(0xff122a20);

  final Color miku;
  final Color muted;
  final Color outline;
  final Color userBubble;
  final Color warm;
  final Color approvalSurface;
  final Color approvalOutline;
  final Color rose;
  final Color roseSoft;
  final Color moss;
  final Color mossSoft;

  /// Eight-step spacing scale. Prefer these over one-off `EdgeInsets` literals
  /// in new or rewritten code.
  static const double space4 = 4;
  static const double space8 = 8;
  static const double space12 = 12;
  static const double space16 = 16;
  static const double space24 = 24;
  static const double space32 = 32;
  static const double space48 = 48;
  static const double space64 = 64;

  static TmTokens of(BuildContext context) =>
      Theme.of(context).extension<TmTokens>() ??
      (Theme.of(context).brightness == Brightness.dark
          ? const TmTokens.dark()
          : const TmTokens.light());

  @override
  TmTokens copyWith({
    Color? miku,
    Color? muted,
    Color? outline,
    Color? userBubble,
    Color? warm,
    Color? approvalSurface,
    Color? approvalOutline,
    Color? rose,
    Color? roseSoft,
    Color? moss,
    Color? mossSoft,
  }) {
    return TmTokens(
      miku: miku ?? this.miku,
      muted: muted ?? this.muted,
      outline: outline ?? this.outline,
      userBubble: userBubble ?? this.userBubble,
      warm: warm ?? this.warm,
      approvalSurface: approvalSurface ?? this.approvalSurface,
      approvalOutline: approvalOutline ?? this.approvalOutline,
      rose: rose ?? this.rose,
      roseSoft: roseSoft ?? this.roseSoft,
      moss: moss ?? this.moss,
      mossSoft: mossSoft ?? this.mossSoft,
    );
  }

  @override
  TmTokens lerp(ThemeExtension<TmTokens>? other, double t) {
    if (other is! TmTokens) return this;
    return TmTokens(
      miku: Color.lerp(miku, other.miku, t)!,
      muted: Color.lerp(muted, other.muted, t)!,
      outline: Color.lerp(outline, other.outline, t)!,
      userBubble: Color.lerp(userBubble, other.userBubble, t)!,
      warm: Color.lerp(warm, other.warm, t)!,
      approvalSurface: Color.lerp(approvalSurface, other.approvalSurface, t)!,
      approvalOutline: Color.lerp(approvalOutline, other.approvalOutline, t)!,
      rose: Color.lerp(rose, other.rose, t)!,
      roseSoft: Color.lerp(roseSoft, other.roseSoft, t)!,
      moss: Color.lerp(moss, other.moss, t)!,
      mossSoft: Color.lerp(mossSoft, other.mossSoft, t)!,
    );
  }
}
