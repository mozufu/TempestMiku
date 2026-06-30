import 'package:flutter/material.dart';

void main() {
  runApp(const MikuApp());
}

class MikuApp extends StatelessWidget {
  const MikuApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'TempestMiku',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xFF4F46E5)),
        useMaterial3: true,
      ),
      home: const MikuHomePage(),
    );
  }
}

class MikuHomePage extends StatelessWidget {
  const MikuHomePage({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('TempestMiku')),
      body: const SafeArea(
        child: Center(
          child: Padding(
            padding: EdgeInsets.all(24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(Icons.cloud_sync, size: 64),
                SizedBox(height: 24),
                Text(
                  'TempestMiku',
                  style: TextStyle(fontSize: 32, fontWeight: FontWeight.w700),
                ),
                SizedBox(height: 12),
                Text(
                  'Flutter Web/PWA client scaffold for the server SSE stream.',
                  textAlign: TextAlign.center,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
