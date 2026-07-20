import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';

abstract interface class MikuThemeModeStore {
  Future<ThemeMode?> read();

  Future<void> write(ThemeMode mode);
}

final class SharedPreferencesMikuThemeModeStore implements MikuThemeModeStore {
  static const preferenceKey = 'tempestmiku.themeMode';

  @override
  Future<ThemeMode?> read() async {
    final preferences = await SharedPreferences.getInstance();
    return switch (preferences.getString(preferenceKey)) {
      'system' => ThemeMode.system,
      'light' => ThemeMode.light,
      'dark' => ThemeMode.dark,
      _ => null,
    };
  }

  @override
  Future<void> write(ThemeMode mode) async {
    final preferences = await SharedPreferences.getInstance();
    await preferences.setString(preferenceKey, mode.name);
  }
}

/// Owns the local-only display preference. The system setting is the
/// fail-closed default when no saved value exists or loading fails.
final class MikuThemeModeController extends ValueNotifier<ThemeMode> {
  MikuThemeModeController({
    MikuThemeModeStore? store,
    ThemeMode initialMode = ThemeMode.system,
  }) : _store = store ?? SharedPreferencesMikuThemeModeStore(),
       super(initialMode);

  final MikuThemeModeStore _store;
  int _revision = 0;
  bool _loaded = false;
  bool _disposed = false;
  Future<void> _writeTail = Future<void>.value();

  bool get loaded => _loaded;

  Future<void> load() async {
    if (_loaded || _disposed) return;
    final loadRevision = _revision;
    ThemeMode? storedMode;
    try {
      storedMode = await _store.read();
    } catch (_) {
      // A local preference must never prevent the chat shell from starting.
      return;
    } finally {
      _loaded = true;
    }
    if (_disposed || loadRevision != _revision || storedMode == null) return;
    value = storedMode;
  }

  Future<void> setThemeMode(ThemeMode mode) async {
    if (_disposed || mode == value) return;
    final previousMode = value;
    final writeRevision = ++_revision;
    value = mode;

    final write = _writeTail.then((_) => _store.write(mode));
    _writeTail = write.onError((_, _) {});
    try {
      await write;
    } catch (_) {
      if (!_disposed && writeRevision == _revision) {
        value = previousMode;
      }
      rethrow;
    }
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
