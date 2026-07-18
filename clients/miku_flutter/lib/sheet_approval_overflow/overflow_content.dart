part of '../main.dart';

// Settings content reused by the right-side drawer. It renders the appearance,
// language, action rows, and connection rows. The project status block lives
// in _ContextContent (top of the same drawer) so it appears exactly once.
class _OverflowContent extends StatefulWidget {
  const _OverflowContent({
    required this.tok,
    required this.copy,
    required this.themeMode,
    required this.onRefresh,
    required this.onPromote,
    required this.onDrive,
    required this.onThemeModeChanged,
    required this.onLanguageToggle,
    required this.onModeSettings,
    this.onServerTarget,
    this.onDisconnect,
  });

  final _Tok tok;
  final _UiCopy copy;
  final ThemeMode themeMode;
  final VoidCallback onRefresh;
  final VoidCallback onPromote;
  final VoidCallback onDrive;
  final ValueChanged<ThemeMode> onThemeModeChanged;
  final VoidCallback onLanguageToggle;
  final VoidCallback onModeSettings;
  final VoidCallback? onServerTarget;
  final VoidCallback? onDisconnect;

  @override
  State<_OverflowContent> createState() => _OverflowContentState();
}

class _OverflowContentState extends State<_OverflowContent> {
  late ThemeMode _themeMode = widget.themeMode;
  late _UiLanguage _language = widget.copy.language;

  _UiCopy get _copy => _UiCopy(_language);

  @override
  void didUpdateWidget(covariant _OverflowContent oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.themeMode != oldWidget.themeMode) {
      _themeMode = widget.themeMode;
    }
    if (widget.copy.language != oldWidget.copy.language) {
      _language = widget.copy.language;
    }
  }

  void _selectTheme(Set<ThemeMode> selection) {
    if (selection.isEmpty) return;
    final mode = selection.first;
    if (_themeMode == mode) return;
    setState(() => _themeMode = mode);
    widget.onThemeModeChanged(mode);
  }

  void _toggleLanguage() {
    setState(() {
      _language = _language == _UiLanguage.en ? _UiLanguage.zh : _UiLanguage.en;
    });
    widget.onLanguageToggle();
  }

  @override
  Widget build(BuildContext context) {
    final tok =
        Theme.of(context).brightness == Brightness.dark
            ? _Tok.dark
            : _Tok.light;
    final copy = _copy;
    final textTheme = Theme.of(context).textTheme;
    final languageName = copy.isZh ? '繁體中文' : 'English';
    final languageHelper = copy.pick(
      'Current language: $languageName. Switch to Traditional Chinese.',
      '目前語言：$languageName。切換為英文。',
    );
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 20),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            copy.pick('Settings', '設定'),
            style: textTheme.titleLarge?.copyWith(color: tok.text),
          ),
          const SizedBox(height: 4),
          Text(
            copy.pick('Appearance and advanced actions', '外觀與進階操作'),
            style: textTheme.bodyMedium?.copyWith(color: tok.muted),
          ),
          const SizedBox(height: 24),
          _SettingsSectionLabel(tok: tok, label: copy.pick('Appearance', '外觀')),
          const SizedBox(height: 8),
          Text(
            copy.pick('Theme', '主題'),
            style: textTheme.titleSmall?.copyWith(
              color: tok.text,
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(height: 8),
          SizedBox(
            width: double.infinity,
            child: SegmentedButton<ThemeMode>(
              segments: [
                ButtonSegment(
                  value: ThemeMode.system,
                  label: Text(copy.pick('System', '系統')),
                ),
                ButtonSegment(
                  value: ThemeMode.light,
                  label: Text(copy.pick('Light', '淺色')),
                ),
                ButtonSegment(
                  value: ThemeMode.dark,
                  label: Text(copy.pick('Dark', '深色')),
                ),
              ],
              selected: {_themeMode},
              onSelectionChanged: _selectTheme,
              showSelectedIcon: false,
              style: ButtonStyle(
                minimumSize: const WidgetStatePropertyAll(Size(48, 48)),
                foregroundColor: WidgetStateProperty.resolveWith(
                  (states) =>
                      states.contains(WidgetState.selected)
                          ? tok.onAccent
                          : tok.text,
                ),
                backgroundColor: WidgetStateProperty.resolveWith(
                  (states) =>
                      states.contains(WidgetState.selected)
                          ? tok.accent
                          : tok.bg,
                ),
                side: WidgetStatePropertyAll(BorderSide(color: tok.border)),
              ),
            ),
          ),
          const SizedBox(height: 10),
          _ActionRow(
            tok: tok,
            icon: Icons.translate_rounded,
            label: copy.pick('Language', '語言'),
            supportingText: languageHelper,
            semanticLabel: copy.languageSemantic,
            trailing: Text(
              copy.nextCode,
              style: textTheme.labelLarge?.copyWith(color: tok.accent),
            ),
            onTap: _toggleLanguage,
          ),
          const SizedBox(height: 24),
          _SettingsSectionLabel(tok: tok, label: copy.pick('Actions', '操作')),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.refresh_rounded,
            label: copy.refreshProject,
            onTap: widget.onRefresh,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.upload_file_rounded,
            label: copy.promoteSession,
            onTap: widget.onPromote,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.folder_outlined,
            label: copy.driveFeed,
            onTap: widget.onDrive,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.tune_rounded,
            label: copy.modeSettings,
            supportingText: copy.pick(
              'Advanced runtime routing',
              '進階 runtime 路由',
            ),
            onTap: widget.onModeSettings,
          ),
          if (widget.onServerTarget != null || widget.onDisconnect != null) ...[
            const SizedBox(height: 24),
            _SettingsSectionLabel(
              tok: tok,
              label: copy.pick('Connection', '連線'),
            ),
            if (widget.onServerTarget != null) ...[
              const SizedBox(height: 8),
              _ActionRow(
                tok: tok,
                icon: Icons.dns_outlined,
                label: copy.serverTarget,
                supportingText: copy.pick(
                  'Pair or change the server',
                  '配對或更換 Server',
                ),
                onTap: widget.onServerTarget!,
              ),
            ],
            if (widget.onDisconnect != null) ...[
              const SizedBox(height: 8),
              _ActionRow(
                tok: tok,
                icon: Icons.logout_rounded,
                label: copy.pick('Disconnect', '中斷連線'),
                supportingText: copy.pick(
                  'Sign out and remove this device credential',
                  '登出並移除此裝置憑證',
                ),
                semanticLabel: copy.pick(
                  'Disconnect from server',
                  '與 Server 中斷連線',
                ),
                foregroundColor: tok.danger,
                onTap: widget.onDisconnect!,
              ),
            ],
          ],
        ],
      ),
    );
  }
}
