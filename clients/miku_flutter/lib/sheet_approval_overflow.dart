part of 'main.dart';

class _ApprovalSheet extends StatefulWidget {
  const _ApprovalSheet({
    required this.approval,
    required this.tok,
    required this.copy,
    required this.accent,
    required this.onOption,
    required this.onApprove,
    required this.onDeny,
  });

  final ApprovalPrompt approval;
  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final void Function(ApprovalOption option) onOption;
  final VoidCallback onApprove, onDeny;

  @override
  State<_ApprovalSheet> createState() => _ApprovalSheetState();
}

class _ApprovalSheetState extends State<_ApprovalSheet> {
  late final int _initialSecs;
  late int _secs;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _initialSecs = math.max(
      1,
      ((widget.approval.timeoutMs ?? 60000) / 1000).ceil(),
    );
    _secs = _initialSecs;
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      setState(() => _secs--);
      if (_secs <= 0) {
        _timer?.cancel();
      }
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final copy = widget.copy;
    final accent = tok.warning;
    final approveColor = widget.accent;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 9, 16, 18),
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 38,
              height: 5,
              decoration: BoxDecoration(
                color: tok.border,
                borderRadius: BorderRadius.circular(999),
              ),
            ),
            const SizedBox(height: 14),
            Row(
              children: [
                Container(
                  width: 40,
                  height: 40,
                  decoration: BoxDecoration(
                    color: accent,
                    borderRadius: BorderRadius.circular(11),
                  ),
                  child: Icon(
                    Icons.warning_amber_rounded,
                    color: _textOn(accent),
                    size: 21,
                  ),
                ),
                const SizedBox(width: 11),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        copy.approvalNeeded,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 17,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(height: 1),
                      Text(
                        copy.approvalHelper,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 12,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 13),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(13),
              decoration: BoxDecoration(
                color: tok.bg,
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(13),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    widget.approval.action,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                      fontFamily: 'monospace',
                      height: 1.5,
                    ),
                  ),
                  if (widget.approval.scope.isNotEmpty) ...[
                    const SizedBox(height: 9),
                    Wrap(
                      spacing: 7,
                      runSpacing: 6,
                      children:
                          widget.approval.scope.entries.map((e) {
                            return Container(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 9,
                                vertical: 3,
                              ),
                              decoration: BoxDecoration(
                                color: tok.surface,
                                borderRadius: BorderRadius.circular(999),
                              ),
                              child: Text(
                                '${e.key}: ${e.value}',
                                style: TextStyle(
                                  color: tok.muted,
                                  fontSize: 11,
                                  fontWeight: FontWeight.w700,
                                ),
                              ),
                            );
                          }).toList(),
                    ),
                  ],
                ],
              ),
            ),
            const SizedBox(height: 13),
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Text(
                  copy.autoDeny,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11.5,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                Text(
                  '${_secs}s',
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 11,
                    fontWeight: FontWeight.w800,
                    fontFamily: 'monospace',
                  ),
                ),
              ],
            ),
            const SizedBox(height: 6),
            ClipRRect(
              borderRadius: BorderRadius.circular(999),
              child: LinearProgressIndicator(
                value: _secs / _initialSecs,
                backgroundColor: tok.border.withValues(alpha: 0.6),
                valueColor: AlwaysStoppedAnimation<Color>(accent),
                minHeight: 5,
              ),
            ),
            const SizedBox(height: 14),
            if (widget.approval.options.isEmpty)
              Row(
                children: [
                  Expanded(
                    child: Semantics(
                      button: true,
                      label: copy.deny,
                      child: Material(
                        color: Colors.transparent,
                        child: InkWell(
                          onTap: widget.onDeny,
                          borderRadius: BorderRadius.circular(13),
                          focusColor: tok.focus.withValues(alpha: 0.18),
                          child: Container(
                            height: 48,
                            decoration: BoxDecoration(
                              color: tok.bg,
                              border: Border.all(color: tok.danger),
                              borderRadius: BorderRadius.circular(13),
                            ),
                            child: Center(
                              child: Text(
                                copy.deny,
                                style: TextStyle(
                                  color: tok.text,
                                  fontSize: 14.5,
                                  fontWeight: FontWeight.w700,
                                ),
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    flex: 3,
                    child: Semantics(
                      button: true,
                      label: copy.approveOnce,
                      child: Material(
                        color: Colors.transparent,
                        child: InkWell(
                          onTap: widget.onApprove,
                          borderRadius: BorderRadius.circular(13),
                          focusColor: tok.focus.withValues(alpha: 0.18),
                          child: Container(
                            height: 48,
                            decoration: BoxDecoration(
                              color: approveColor,
                              borderRadius: BorderRadius.circular(13),
                            ),
                            child: Center(
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  Icon(
                                    Icons.check,
                                    color: _textOn(approveColor),
                                    size: 17,
                                  ),
                                  const SizedBox(width: 7),
                                  Text(
                                    copy.approveOnce,
                                    style: TextStyle(
                                      color: _textOn(approveColor),
                                      fontSize: 14.5,
                                      fontWeight: FontWeight.w800,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              )
            else
              Column(
                children:
                    widget.approval.options.map((option) {
                      final isReject =
                          option.kind.startsWith('reject') ||
                          option.kind.startsWith('deny');
                      final buttonColor = isReject ? tok.bg : approveColor;
                      final textColor =
                          isReject ? tok.text : _textOn(approveColor);
                      final rawLabel =
                          option.name.isEmpty ? option.optionId : option.name;
                      final label =
                          isReject
                              ? copy.deny
                              : rawLabel.toLowerCase().contains('allow')
                              ? copy.approveOnce
                              : rawLabel;
                      return Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: Semantics(
                          button: true,
                          label: label,
                          child: Material(
                            color: Colors.transparent,
                            child: InkWell(
                              onTap: () => widget.onOption(option),
                              borderRadius: BorderRadius.circular(13),
                              focusColor: tok.focus.withValues(alpha: 0.18),
                              child: Container(
                                width: double.infinity,
                                constraints: const BoxConstraints(
                                  minHeight: 48,
                                ),
                                decoration: BoxDecoration(
                                  color: buttonColor,
                                  border: Border.all(
                                    color: isReject ? tok.danger : approveColor,
                                  ),
                                  borderRadius: BorderRadius.circular(13),
                                ),
                                child: Center(
                                  child: Text(
                                    label,
                                    style: TextStyle(
                                      color: textColor,
                                      fontSize: 14,
                                      fontWeight: FontWeight.w800,
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                      );
                    }).toList(),
              ),
          ],
        ),
      ),
    );
  }
}

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
          _SettingsSectionLabel(
            tok: tok,
            label: copy.pick('Appearance', '外觀'),
          ),
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
          if (widget.onServerTarget != null ||
              widget.onDisconnect != null) ...[
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

class _SettingsSectionLabel extends StatelessWidget {
  const _SettingsSectionLabel({required this.tok, required this.label});

  final _Tok tok;
  final String label;

  @override
  Widget build(BuildContext context) {
    return Text(
      label,
      style: Theme.of(
        context,
      ).textTheme.labelLarge?.copyWith(color: tok.muted, letterSpacing: 0.4),
    );
  }
}

class _ActionRow extends StatelessWidget {
  const _ActionRow({
    required this.tok,
    required this.icon,
    required this.label,
    required this.onTap,
    this.supportingText,
    this.semanticLabel,
    this.trailing,
    this.foregroundColor,
  });

  final _Tok tok;
  final IconData icon;
  final String label;
  final VoidCallback onTap;
  final String? supportingText;
  final String? semanticLabel;
  final Widget? trailing;
  final Color? foregroundColor;

  @override
  Widget build(BuildContext context) {
    final color = foregroundColor ?? tok.text;
    final textTheme = Theme.of(context).textTheme;
    return Semantics(
      button: true,
      excludeSemantics: true,
      label: semanticLabel ?? label,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(16),
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 56),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
              decoration: BoxDecoration(
                color: tok.raised,
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(16),
              ),
              child: Row(
                children: [
                  Icon(icon, color: foregroundColor ?? tok.muted, size: 22),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          label,
                          style: textTheme.titleSmall?.copyWith(
                            color: color,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                        if (supportingText != null) ...[
                          const SizedBox(height: 2),
                          Text(
                            supportingText!,
                            style: textTheme.bodySmall?.copyWith(
                              color:
                                  foregroundColor?.withValues(alpha: 0.88) ??
                                  tok.muted,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  if (trailing != null) ...[
                    const SizedBox(width: 10),
                    trailing!,
                  ],
                  const SizedBox(width: 4),
                  Icon(
                    Icons.chevron_right_rounded,
                    color: foregroundColor ?? tok.muted,
                    size: 22,
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
