part of 'main.dart';

// ─── App ──────────────────────────────────────────────────────────────────────

class MikuApp extends StatefulWidget {
  const MikuApp({
    super.key,
    required this.client,
    this.notifications,
    this.shareImports,
    this.themeController,
  });

  final MikuSessionClient client;
  final MikuNotificationService? notifications;
  final MikuShareImportService? shareImports;
  final MikuThemeController? themeController;

  @override
  State<MikuApp> createState() => _MikuAppState();
}

class _MikuAppState extends State<MikuApp> {
  late final MikuThemeController _themeController =
      widget.themeController ?? MikuThemeController();
  late final bool _ownsThemeController = widget.themeController == null;

  @override
  void initState() {
    super.initState();
    if (_ownsThemeController) unawaited(_themeController.load());
  }

  @override
  void dispose() {
    if (_ownsThemeController) _themeController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _themeController,
      builder: (context, _) {
        return MaterialApp(
          title: 'Tempest Miku',
          debugShowCheckedModeBanner: false,
          theme: MikuTheme.light,
          darkTheme: MikuTheme.dark,
          themeMode: _themeController.mode,
          // Legacy feature surfaces still use the compatibility token layer;
          // switch atomically until every surface consumes interpolated tokens.
          themeAnimationDuration: Duration.zero,
          builder: (context, child) {
            return MikuThemeScope(
              controller: _themeController,
              child: child ?? const SizedBox.shrink(),
            );
          },
          home: MikuHomePage(
            client: widget.client,
            notifications: widget.notifications ?? createNotificationService(),
            shareImports: widget.shareImports ?? createShareImportService(),
          ),
        );
      },
    );
  }
}
