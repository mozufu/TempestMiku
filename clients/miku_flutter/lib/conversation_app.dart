import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';

import 'asr/local_asr_engine.dart';
import 'asr/local_asr_model.dart';
import 'conversation_screen.dart';
import 'notification_service.dart';
import 'session_models.dart';
import 'share_import_service.dart';
import 'theme_mode_controller.dart';
import 'voice_capture_service.dart';

class TempestMikuApp extends StatefulWidget {
  const TempestMikuApp({
    required this.client,
    this.themeMode,
    this.themeModeController,
    this.now,
    this.shareImports,
    this.voiceCapture,
    this.localAsrWorkers,
    this.localAsrModels,
    this.notifications,
    this.voiceInferenceTimeout = const Duration(seconds: 45),
    super.key,
  });

  final MikuSessionClient client;
  final ThemeMode? themeMode;
  final MikuThemeModeController? themeModeController;
  final DateTime Function()? now;
  final MikuShareImportService? shareImports;
  final MikuVoiceCaptureService? voiceCapture;
  final LocalAsrWorkerFactory? localAsrWorkers;
  final LocalAsrModelManager? localAsrModels;
  final MikuNotificationService? notifications;
  final Duration voiceInferenceTimeout;

  @override
  State<TempestMikuApp> createState() => _TempestMikuAppState();
}

class _TempestMikuAppState extends State<TempestMikuApp> {
  late final bool _ownsThemeModeController = widget.themeModeController == null;
  late final MikuThemeModeController _themeModeController =
      widget.themeModeController ??
      MikuThemeModeController(
        initialMode: widget.themeMode ?? ThemeMode.system,
      );

  @override
  void initState() {
    super.initState();
    if (widget.themeMode == null) {
      unawaited(_themeModeController.load());
    }
  }

  @override
  void dispose() {
    if (_ownsThemeModeController) _themeModeController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<ThemeMode>(
      valueListenable: _themeModeController,
      builder:
          (context, themeMode, _) => MaterialApp(
            title: 'TempestMiku',
            debugShowCheckedModeBanner: false,
            themeMode: themeMode,
            localizationsDelegates: GlobalMaterialLocalizations.delegates,
            supportedLocales: const [
              Locale('zh', 'TW'),
              Locale('zh'),
              Locale('en'),
            ],
            locale: const Locale('zh', 'TW'),
            theme: _theme(Brightness.light),
            darkTheme: _theme(Brightness.dark),
            home: ConversationScreen(
              client: widget.client,
              themeModeController: _themeModeController,
              now: widget.now,
              shareImports: widget.shareImports,
              voiceCapture: widget.voiceCapture,
              localAsrWorkers: widget.localAsrWorkers,
              localAsrModels: widget.localAsrModels,
              notifications: widget.notifications,
              voiceInferenceTimeout: widget.voiceInferenceTimeout,
            ),
          ),
    );
  }
}

ThemeData _theme(Brightness brightness) {
  final isDark = brightness == Brightness.dark;
  const mikuCyan = Color(0xff5fd0c5);
  final scheme = ColorScheme.fromSeed(
    seedColor: mikuCyan,
    brightness: brightness,
  ).copyWith(
    primary: isDark ? mikuCyan : const Color(0xff167f78),
    onPrimary: isDark ? const Color(0xff09201e) : Colors.white,
    surface: isDark ? const Color(0xff11191e) : const Color(0xfffbfaf7),
    onSurface: isDark ? const Color(0xffe8eff1) : const Color(0xff182126),
    error: isDark ? const Color(0xffffb4ab) : const Color(0xffba1a1a),
  );

  return ThemeData(
    useMaterial3: true,
    brightness: brightness,
    colorScheme: scheme,
    scaffoldBackgroundColor:
        isDark ? const Color(0xff0b1115) : const Color(0xfff4f3ef),
    dividerColor: isDark ? const Color(0xff253138) : const Color(0xffdce1df),
    textTheme: ThemeData(brightness: brightness).textTheme
        .copyWith(
          bodyLarge: const TextStyle(fontSize: 16, height: 1.55),
          bodyMedium: const TextStyle(fontSize: 14, height: 1.45),
          labelLarge: const TextStyle(
            fontSize: 14,
            fontWeight: FontWeight.w600,
          ),
        )
        .apply(bodyColor: scheme.onSurface, displayColor: scheme.onSurface),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: isDark ? const Color(0xff151f25) : const Color(0xffffffff),
      hintStyle: TextStyle(
        color: isDark ? const Color(0xff8d9ba1) : const Color(0xff69777c),
      ),
      contentPadding: const EdgeInsets.fromLTRB(18, 14, 10, 14),
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(24),
        borderSide: BorderSide(
          color: isDark ? const Color(0xff2b3940) : const Color(0xffd7dddb),
        ),
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(24),
        borderSide: BorderSide(
          color: isDark ? const Color(0xff2b3940) : const Color(0xffd7dddb),
        ),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(24),
        borderSide: BorderSide(color: scheme.primary, width: 1.5),
      ),
    ),
  );
}
