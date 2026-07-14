import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/main.dart';
import 'package:miku_flutter/session_client_stub.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('MikuThemeController', () {
    test('loads every persisted theme mode', () async {
      for (final mode in ThemeMode.values) {
        SharedPreferences.setMockInitialValues({
          MikuThemeController.preferenceKey: mode.name,
        });
        final controller = MikuThemeController();
        addTearDown(controller.dispose);

        await controller.load();

        expect(controller.loaded, isTrue, reason: 'failed to load $mode');
        expect(controller.mode, mode, reason: 'failed to decode $mode');
      }
    });

    test('persists light, dark, and restored system mode', () async {
      SharedPreferences.setMockInitialValues({});
      final controller = MikuThemeController();
      addTearDown(controller.dispose);

      for (final mode in const [ThemeMode.light, ThemeMode.dark]) {
        await controller.setMode(mode);
        final preferences = await SharedPreferences.getInstance();
        expect(controller.mode, mode);
        expect(
          preferences.getString(MikuThemeController.preferenceKey),
          mode.name,
        );
      }

      await controller.clearOverride();
      final preferences = await SharedPreferences.getInstance();
      expect(controller.mode, ThemeMode.system);
      expect(
        preferences.getString(MikuThemeController.preferenceKey),
        ThemeMode.system.name,
      );
    });

    test('unknown persisted values fail closed to the system theme', () async {
      SharedPreferences.setMockInitialValues({
        MikuThemeController.preferenceKey: 'storm',
      });
      final controller = MikuThemeController(initialMode: ThemeMode.dark);
      addTearDown(controller.dispose);

      await controller.load();

      expect(controller.loaded, isTrue);
      expect(controller.mode, ThemeMode.system);
    });
  });

  testWidgets('MikuApp honors and reacts to a supplied theme controller', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({
      MikuThemeController.preferenceKey: ThemeMode.dark.name,
    });
    final controller = MikuThemeController(initialMode: ThemeMode.light);
    addTearDown(controller.dispose);

    await tester.pumpWidget(
      MikuApp(client: ScriptedMikuClient(), themeController: controller),
    );
    await tester.pump();

    MaterialApp app = tester.widget(find.byType(MaterialApp));
    expect(app.themeMode, ThemeMode.light);
    expect(
      MikuThemeScope.controllerOf(tester.element(find.byType(MikuHomePage))),
      same(controller),
    );

    await controller.setMode(ThemeMode.dark);
    await tester.pump();

    app = tester.widget(find.byType(MaterialApp));
    expect(app.themeMode, ThemeMode.dark);
  });

  testWidgets('storm-cat brand badge exposes one accessible image label', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();

    await tester.pumpWidget(
      MaterialApp(
        theme: MikuTheme.light,
        home: const Scaffold(
          body: Center(
            child: MikuBrandBadge(semanticLabel: 'Tempest Miku companion'),
          ),
        ),
      ),
    );

    expect(find.bySemanticsLabel('Tempest Miku companion'), findsOneWidget);
    semantics.dispose();
  });
}
