import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/theme_mode_controller.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('defaults to system and persists a selected mode', () async {
    final controller = MikuThemeModeController();
    addTearDown(controller.dispose);

    await controller.load();
    expect(controller.value, ThemeMode.system);

    await controller.setThemeMode(ThemeMode.dark);
    final preferences = await SharedPreferences.getInstance();
    expect(
      preferences.getString(SharedPreferencesMikuThemeModeStore.preferenceKey),
      'dark',
    );

    final restored = MikuThemeModeController();
    addTearDown(restored.dispose);
    await restored.load();
    expect(restored.value, ThemeMode.dark);
  });

  test('ignores invalid saved values and read failures', () async {
    SharedPreferences.setMockInitialValues({
      SharedPreferencesMikuThemeModeStore.preferenceKey: 'sepia',
    });
    final invalid = MikuThemeModeController();
    addTearDown(invalid.dispose);
    await invalid.load();
    expect(invalid.value, ThemeMode.system);

    final failing = MikuThemeModeController(
      store: const _FailingThemeModeStore(),
    );
    addTearDown(failing.dispose);
    await failing.load();
    expect(failing.value, ThemeMode.system);
    expect(failing.loaded, isTrue);
  });

  test('restores the prior mode when persistence fails', () async {
    final controller = MikuThemeModeController(
      store: const _FailingThemeModeStore(failWrite: true),
      initialMode: ThemeMode.light,
    );
    addTearDown(controller.dispose);

    await expectLater(
      controller.setThemeMode(ThemeMode.dark),
      throwsA(isA<StateError>()),
    );
    expect(controller.value, ThemeMode.light);
  });

  testWidgets('loads the saved theme and changes it from Settings', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(375, 812);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    SharedPreferences.setMockInitialValues({
      SharedPreferencesMikuThemeModeStore.preferenceKey: 'dark',
    });

    await tester.pumpWidget(TempestMikuApp(client: ScriptedMikuClient()));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(
      tester.widget<MaterialApp>(find.byType(MaterialApp)).themeMode,
      ThemeMode.dark,
    );

    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('drawer-settings')));
    await tester.pumpAndSettle();

    expect(find.byKey(const Key('theme-mode-chooser')), findsOneWidget);
    expect(find.bySemanticsLabel('顯示主題'), findsOneWidget);
    expect(
      tester.getSize(find.byKey(const Key('theme-mode-chooser'))).height,
      greaterThanOrEqualTo(44),
    );

    await tester.tap(find.byKey(const Key('theme-mode-light')));
    await tester.pumpAndSettle();

    expect(
      tester.widget<MaterialApp>(find.byType(MaterialApp)).themeMode,
      ThemeMode.light,
    );
    final preferences = await SharedPreferences.getInstance();
    expect(
      preferences.getString(SharedPreferencesMikuThemeModeStore.preferenceKey),
      'light',
    );
  });
}

final class _FailingThemeModeStore implements MikuThemeModeStore {
  const _FailingThemeModeStore({this.failWrite = false});

  final bool failWrite;

  @override
  Future<ThemeMode?> read() =>
      Future<ThemeMode?>.error(StateError('preference read failed'));

  @override
  Future<void> write(ThemeMode mode) {
    if (failWrite) {
      return Future<void>.error(StateError('preference write failed'));
    }
    return Future<void>.value();
  }
}
