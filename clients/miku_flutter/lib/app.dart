part of 'main.dart';

// ─── App ──────────────────────────────────────────────────────────────────────

class MikuApp extends StatelessWidget {
  const MikuApp({super.key, required this.client});

  final MikuSessionClient client;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'TempestMiku',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(useMaterial3: true),
      home: MikuHomePage(client: client),
    );
  }
}
