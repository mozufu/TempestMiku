part of 'main.dart';

// ─── App ──────────────────────────────────────────────────────────────────────

class MikuApp extends StatelessWidget {
  const MikuApp({super.key, required this.client});

  final MikuSessionClient client;
  static const _fontFallbacks = [
    '.SF Pro Text',
    'Segoe UI',
    'Roboto',
    'MikuCjkUi',
    'PingFang TC',
    'Noto Sans CJK TC',
    'Noto Sans TC',
    'Microsoft JhengHei',
    'Arial',
    'sans-serif',
  ];

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'TempestMiku',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        useMaterial3: true,
        fontFamily: _fontFallbacks.first,
        fontFamilyFallback: _fontFallbacks.skip(1).toList(),
        colorScheme: ColorScheme.fromSeed(
          seedColor: _Tok.dark.accent,
          brightness: Brightness.dark,
        ),
        splashFactory: InkSparkle.splashFactory,
      ),
      home: MikuHomePage(client: client),
    );
  }
}
