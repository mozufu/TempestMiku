part of 'main.dart';

class MikuThemeController extends ChangeNotifier {
  MikuThemeController({ThemeMode initialMode = ThemeMode.system})
    : _mode = initialMode;

  static const preferenceKey = 'tempest_miku.ui.theme_mode.v1';

  ThemeMode _mode;
  bool _loaded = false;

  ThemeMode get mode => _mode;
  bool get loaded => _loaded;

  Future<void> load() async {
    if (_loaded) return;
    try {
      final preferences = await SharedPreferences.getInstance();
      final storedMode = _decode(preferences.getString(preferenceKey));
      if (storedMode != _mode) {
        _mode = storedMode;
        notifyListeners();
      }
    } catch (_) {
      // Preferences are a progressive enhancement. An unavailable store must
      // never keep the companion from opening with the system theme.
    } finally {
      _loaded = true;
    }
  }

  Future<void> setMode(ThemeMode mode) async {
    if (_mode != mode) {
      _mode = mode;
      notifyListeners();
    }
    try {
      final preferences = await SharedPreferences.getInstance();
      await preferences.setString(preferenceKey, mode.name);
    } catch (_) {
      // Keep the selected mode for this process even if persistence fails.
    }
  }

  Future<void> clearOverride() => setMode(ThemeMode.system);

  static ThemeMode _decode(String? value) {
    return switch (value) {
      'light' => ThemeMode.light,
      'dark' => ThemeMode.dark,
      _ => ThemeMode.system,
    };
  }
}

class MikuThemeScope extends InheritedNotifier<MikuThemeController> {
  const MikuThemeScope({
    super.key,
    required MikuThemeController controller,
    required super.child,
  }) : super(notifier: controller);

  static MikuThemeController controllerOf(BuildContext context) {
    final scope = context.dependOnInheritedWidgetOfExactType<MikuThemeScope>();
    assert(scope != null, 'No MikuThemeScope found in this context.');
    return scope!.notifier!;
  }

  static MikuThemeController? maybeControllerOf(BuildContext context) {
    return context
        .dependOnInheritedWidgetOfExactType<MikuThemeScope>()
        ?.notifier;
  }
}
