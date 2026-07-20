part of 'conversation_screen.dart';

enum _SettingsResult { loggedOut, paired }

class _SettingsSheet extends StatefulWidget {
  const _SettingsSheet({
    required this.client,
    required this.themeModeController,
    required this.voiceSupported,
    required this.initialVoiceModelStatus,
    required this.initialVoiceCatalog,
    required this.initialVoiceSelection,
    required this.onPrepareDeviceAuthorityChange,
    required this.onAuthorityChangeCommitted,
    required this.onAuthorityChangeAborted,
    required this.onRefreshVoiceModel,
    required this.onInstallVoiceModel,
    required this.onDeleteVoiceModel,
    required this.onRefreshVoiceCatalog,
    required this.onSelectVoiceEngine,
    this.notificationSettingsPanel,
  });

  final MikuSessionClient client;
  final MikuThemeModeController themeModeController;
  final bool voiceSupported;
  final LocalAsrModelStatus? initialVoiceModelStatus;
  final VoiceAsrEngineCatalog initialVoiceCatalog;
  final VoiceAsrEngineKind initialVoiceSelection;
  final Future<bool> Function(bool preserveNotificationIntent)
  onPrepareDeviceAuthorityChange;
  final Future<void> Function() onAuthorityChangeCommitted;
  final Future<void> Function() onAuthorityChangeAborted;
  final Future<LocalAsrModelStatus?> Function() onRefreshVoiceModel;
  final Future<LocalAsrModelStatus> Function() onInstallVoiceModel;
  final Future<LocalAsrModelStatus> Function() onDeleteVoiceModel;
  final Future<VoiceAsrEngineCatalog> Function() onRefreshVoiceCatalog;
  final Future<bool> Function(VoiceAsrEngineKind) onSelectVoiceEngine;
  final Widget? notificationSettingsPanel;

  @override
  State<_SettingsSheet> createState() => _SettingsSheetState();
}

class _SettingsSheetState extends State<_SettingsSheet> {
  final TextEditingController _pairingLinkController = TextEditingController();
  ServerReadiness? _readiness;
  ServerDiagnostics? _diagnostics;
  List<AuthDevice>? _devices;
  String? _currentDeviceId;
  String? _readinessError;
  String? _diagnosticsError;
  String? _devicesError;
  String? _revokingDeviceId;
  bool _loggingOut = false;
  bool _creatingPairingCode = false;
  bool _pairing = false;
  String? _pairingError;
  String? _pairingNotice;
  late LocalAsrModelStatus? _voiceModelStatus = widget.initialVoiceModelStatus;
  late VoiceAsrEngineCatalog _voiceCatalog = widget.initialVoiceCatalog;
  late VoiceAsrEngineKind _voiceSelection = widget.initialVoiceSelection;
  bool _voiceSettingsLoading = false;
  bool _voiceModelOperation = false;
  String? _voiceSettingsError;
  bool _savingThemeMode = false;
  String? _themeModeError;

  void _voiceSetState(VoidCallback update) => setState(update);

  Future<void> _scanPairingQr() async {
    if (_pairing) return;
    final pairingLink = await Navigator.of(context).push<String>(
      MaterialPageRoute(builder: (_) => const PairingScannerPage()),
    );
    if (!mounted || pairingLink == null) return;
    _pairingLinkController.value = TextEditingValue(
      text: pairingLink,
      selection: TextSelection.collapsed(offset: pairingLink.length),
    );
    setState(() {
      _pairingError = null;
      _pairingNotice = '已讀取 QR；尚未配對。請檢查目標後再確認。';
    });
  }

  Future<void> _selectThemeMode(ThemeMode mode) async {
    if (_savingThemeMode || mode == widget.themeModeController.value) return;
    setState(() {
      _savingThemeMode = true;
      _themeModeError = null;
    });
    try {
      await widget.themeModeController.setThemeMode(mode);
    } catch (_) {
      if (mounted) {
        setState(() {
          _themeModeError = '顯示主題沒有保存，已恢復先前選擇。請再試一次。';
        });
      }
    } finally {
      if (mounted) setState(() => _savingThemeMode = false);
    }
  }

  @override
  void initState() {
    super.initState();
    unawaited(_loadDiagnostics());
    unawaited(_loadDevices());
    if (widget.voiceSupported) unawaited(_loadVoiceSettings());
  }

  @override
  void dispose() {
    _pairingLinkController.dispose();
    super.dispose();
  }

  Future<void> _pairThisDevice() async {
    final client = widget.client;
    if (client is! ServerTargetClient || _pairing) return;
    final targetClient = client as ServerTargetClient;

    late final MikuPairingTarget target;
    try {
      target = pairingTargetFromLink(_pairingLinkController.text);
    } on FormatException {
      setState(() => _pairingError = '這不是有效的一次性 TempestMiku 配對連結。');
      return;
    }

    final confirmed = await showDialog<bool>(
      context: context,
      builder:
          (context) => AlertDialog(
            title: const Text('確認配對目標'),
            content: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 480),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const Text('一次性連結會授權這台裝置。請先確認這是你管理的伺服器。'),
                  const SizedBox(height: 16),
                  _PairingTargetRow(label: '協定', value: target.scheme),
                  _PairingTargetRow(label: '主機', value: target.host),
                  _PairingTargetRow(
                    label: '連接埠',
                    value: '${target.effectivePort}',
                  ),
                  _PairingTargetRow(
                    label: '裝置名稱',
                    value: targetClient.pairingDeviceName(),
                  ),
                  const SizedBox(height: 10),
                  SelectableText(
                    target.origin,
                    key: const Key('pairing-target-origin'),
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(fontFamily: 'monospace'),
                  ),
                ],
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(false),
                child: const Text('取消'),
              ),
              FilledButton(
                key: const Key('confirm-pair-device'),
                onPressed: () => Navigator.of(context).pop(true),
                child: const Text('確認並配對'),
              ),
            ],
          ),
    );
    if (confirmed != true || !mounted) return;

    setState(() {
      _pairing = true;
      _pairingError = null;
      _pairingNotice = null;
    });
    var authorityPrepared = false;
    var paired = false;
    try {
      final cleaned = await widget.onPrepareDeviceAuthorityChange(true);
      if (!cleaned) {
        if (!mounted) return;
        setState(() {
          _pairing = false;
          _pairingError = '裝置內仍有錄音或通知工作尚未安全清除，因此沒有變更配對伺服器。請再試一次。';
        });
        return;
      }
      authorityPrepared = true;
      await targetClient.pairWithCode(target);
      paired = true;
      await widget.onAuthorityChangeCommitted();
      if (!mounted) return;
      Navigator.of(context).pop(_SettingsResult.paired);
    } catch (_) {
      if (authorityPrepared && !paired) {
        await widget.onAuthorityChangeAborted();
      }
      if (!mounted) return;
      setState(() {
        _pairing = false;
        _pairingError = '配對沒有完成。請確認連結尚未失效，並再試一次。';
      });
    }
  }

  Future<void> _loadDiagnostics() async {
    setState(() {
      _readinessError = null;
      _diagnosticsError = null;
    });
    ServerReadiness? readiness;
    ServerDiagnostics? diagnostics;
    var readinessFailed = false;
    var diagnosticsFailed = false;
    await Future.wait([
      () async {
        try {
          readiness = await widget.client.serverReadiness();
        } catch (_) {
          readinessFailed = true;
        }
      }(),
      () async {
        try {
          diagnostics = await widget.client.serverDiagnostics();
        } catch (_) {
          diagnosticsFailed = true;
        }
      }(),
    ]);
    if (!mounted) return;
    setState(() {
      if (readiness != null) _readiness = readiness;
      if (diagnostics != null) _diagnostics = diagnostics;
      if (readinessFailed) _readinessError = '伺服器就緒狀態暫時讀不到。';
      if (diagnosticsFailed) _diagnosticsError = '伺服器佇列診斷暫時讀不到。';
    });
  }

  Future<void> _loadDevices() async {
    setState(() => _devicesError = null);
    try {
      String? currentDeviceId;
      final CurrentAuthDeviceClient? currentDeviceClient =
          widget.client is CurrentAuthDeviceClient
              ? widget.client as CurrentAuthDeviceClient
              : null;
      final devicesFuture = widget.client.authDevices();
      if (currentDeviceClient != null) {
        try {
          currentDeviceId = await currentDeviceClient.currentAuthDeviceId();
        } catch (_) {
          // Device inventory remains useful when the installation-local hint
          // is unavailable; never guess which authenticated row is current.
        }
      }
      final devices = await devicesFuture;
      if (!mounted) return;
      setState(() {
        _devices = devices;
        _currentDeviceId = currentDeviceId;
      });
    } catch (_) {
      if (!mounted) return;
      setState(() => _devicesError = '已配對裝置暫時讀不到。');
    }
  }

  Future<void> _revokeDevice(AuthDevice device) async {
    if (_revokingDeviceId != null ||
        !device.isActive ||
        device.id == _currentDeviceId) {
      return;
    }
    final confirmed = await showDialog<bool>(
      context: context,
      builder:
          (context) => AlertDialog(
            title: const Text('撤銷裝置？'),
            content: Text(
              '${device.name} 將立即失去 API、事件串流與通知權限。裝置上的本機資料不會被遠端刪除。',
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(false),
                child: const Text('取消'),
              ),
              FilledButton(
                key: const Key('confirm-device-revoke'),
                onPressed: () => Navigator.of(context).pop(true),
                child: const Text('撤銷權限'),
              ),
            ],
          ),
    );
    if (confirmed != true || !mounted) return;
    setState(() {
      _revokingDeviceId = device.id;
      _devicesError = null;
    });
    try {
      await widget.client.revokeAuthDevice(device.id);
      if (!mounted) return;
      setState(() {
        _devices = [
          for (final item in _devices ?? const <AuthDevice>[])
            if (item.id != device.id) item,
        ];
      });
    } catch (_) {
      if (!mounted) return;
      setState(() => _devicesError = '沒有撤銷這台裝置，請再試一次。');
    } finally {
      if (mounted) setState(() => _revokingDeviceId = null);
    }
  }

  Future<void> _createPairingCode() async {
    if (_creatingPairingCode) return;
    setState(() {
      _creatingPairingCode = true;
      _devicesError = null;
    });
    try {
      final pairing = await widget.client.createPairingCode();
      if (!mounted) return;
      await showDialog<void>(
        context: context,
        builder:
            (context) => AlertDialog(
              title: const Text('配對新裝置'),
              content: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 520),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const Text('在新裝置貼上這個一次性連結。連結會在五分鐘後失效。'),
                    const SizedBox(height: 14),
                    DecoratedBox(
                      decoration: BoxDecoration(
                        border: Border.all(color: _Palette.of(context).outline),
                        borderRadius: BorderRadius.circular(12),
                      ),
                      child: Padding(
                        padding: const EdgeInsets.all(12),
                        child: SelectableText(
                          pairing.pairingLink,
                          key: const Key('pairing-link'),
                          style: const TextStyle(fontFamily: 'monospace'),
                        ),
                      ),
                    ),
                    const SizedBox(height: 8),
                    Text(
                      '到期：${_friendlyTimestamp(pairing.expiresAt)}',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
              actions: [
                TextButton.icon(
                  key: const Key('copy-pairing-link'),
                  onPressed: () async {
                    await Clipboard.setData(
                      ClipboardData(text: pairing.pairingLink),
                    );
                  },
                  icon: const Icon(Icons.copy_rounded),
                  label: const Text('複製連結'),
                ),
                FilledButton(
                  onPressed: () => Navigator.of(context).pop(),
                  child: const Text('完成'),
                ),
              ],
            ),
      );
    } catch (_) {
      if (!mounted) return;
      setState(() => _devicesError = '無法建立配對連結，請再試一次。');
    } finally {
      if (mounted) setState(() => _creatingPairingCode = false);
    }
  }

  Future<void> _logout() async {
    if (_loggingOut) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder:
          (context) => AlertDialog(
            title: const Text('登出這台裝置？'),
            content: const Text('這會撤銷目前裝置的伺服器權限並清除本機憑證。之後需要重新配對。'),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(false),
                child: const Text('取消'),
              ),
              FilledButton(
                key: const Key('confirm-logout'),
                onPressed: () => Navigator.of(context).pop(true),
                child: const Text('登出'),
              ),
            ],
          ),
    );
    if (confirmed != true || !mounted) return;
    setState(() => _loggingOut = true);
    var authorityPrepared = false;
    var loggedOut = false;
    try {
      final cleaned = await widget.onPrepareDeviceAuthorityChange(false);
      if (!cleaned) {
        if (!mounted) return;
        setState(() {
          _loggingOut = false;
          _devicesError = '裝置內仍有錄音或通知工作尚未安全清除，因此沒有登出。請再試一次。';
        });
        return;
      }
      authorityPrepared = true;
      await widget.client.logout();
      loggedOut = true;
      await widget.onAuthorityChangeCommitted();
      if (!mounted) return;
      Navigator.of(context).pop(_SettingsResult.loggedOut);
    } catch (_) {
      if (authorityPrepared && !loggedOut) {
        await widget.onAuthorityChangeAborted();
      }
      if (!mounted) return;
      setState(() {
        _loggingOut = false;
        _devicesError = '登出沒有完成，裝置仍保持登入。';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return PopScope(
      canPop: !_voiceModelOperation && !_pairing && !_loggingOut,
      child: FractionallySizedBox(
        key: const Key('settings-sheet'),
        heightFactor: 0.92,
        child: LayoutBuilder(
          builder:
              (context, constraints) => Center(
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 680),
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Text(
                          '設定',
                          key: const Key('settings-title'),
                          style: Theme.of(context).textTheme.titleLarge
                              ?.copyWith(fontWeight: FontWeight.w600),
                        ),
                        const SizedBox(height: 4),
                        Text(
                          '伺服器狀態與裝置權限',
                          style: Theme.of(
                            context,
                          ).textTheme.bodySmall?.copyWith(color: palette.muted),
                        ),
                        const SizedBox(height: 14),
                        Expanded(
                          child: ListView(
                            key: const Key('settings-list'),
                            children: [
                              _SettingsSection(
                                title: '外觀',
                                action: const SizedBox.shrink(),
                                child: ValueListenableBuilder<ThemeMode>(
                                  valueListenable: widget.themeModeController,
                                  builder:
                                      (context, mode, _) => _ThemeModeSettings(
                                        mode: mode,
                                        saving: _savingThemeMode,
                                        error: _themeModeError,
                                        onChanged: _selectThemeMode,
                                      ),
                                ),
                              ),
                              const SizedBox(height: 20),
                              if (widget.client is ServerTargetClient) ...[
                                _SettingsSection(
                                  title: '配對這台裝置',
                                  action: const SizedBox.shrink(),
                                  child: Column(
                                    crossAxisAlignment:
                                        CrossAxisAlignment.stretch,
                                    children: [
                                      Text(
                                        kIsWeb
                                            ? '貼上由已登入裝置建立的一次性連結；送出前會先讓你核對伺服器。'
                                            : '掃描或貼上由已登入裝置建立的一次性連結；送出前會先讓你核對伺服器。',
                                        style:
                                            Theme.of(
                                              context,
                                            ).textTheme.bodySmall,
                                      ),
                                      const SizedBox(height: 10),
                                      if (!kIsWeb) ...[
                                        OutlinedButton.icon(
                                          key: const Key('scan-pairing-qr'),
                                          onPressed:
                                              _pairing ? null : _scanPairingQr,
                                          style: OutlinedButton.styleFrom(
                                            minimumSize: const Size(0, 48),
                                          ),
                                          icon: const Icon(
                                            Icons.qr_code_scanner_rounded,
                                          ),
                                          label: const Text('掃描配對 QR'),
                                        ),
                                        const SizedBox(height: 10),
                                      ],
                                      TextField(
                                        key: const Key('pairing-link-input'),
                                        controller: _pairingLinkController,
                                        enabled: !_pairing,
                                        minLines: 2,
                                        maxLines: 3,
                                        autocorrect: false,
                                        enableSuggestions: false,
                                        decoration: InputDecoration(
                                          labelText: '一次性配對連結',
                                          hintText: 'tempestmiku://pair?...',
                                          errorText: _pairingError,
                                          border: const OutlineInputBorder(),
                                        ),
                                        onChanged: (_) {
                                          if (_pairingError != null ||
                                              _pairingNotice != null) {
                                            setState(() {
                                              _pairingError = null;
                                              _pairingNotice = null;
                                            });
                                          }
                                        },
                                      ),
                                      if (_pairingNotice != null) ...[
                                        const SizedBox(height: 8),
                                        Semantics(
                                          liveRegion: true,
                                          child: Text(
                                            _pairingNotice!,
                                            key: const Key(
                                              'pairing-scan-review-notice',
                                            ),
                                            style: Theme.of(
                                              context,
                                            ).textTheme.bodySmall?.copyWith(
                                              color:
                                                  Theme.of(
                                                    context,
                                                  ).colorScheme.primary,
                                            ),
                                          ),
                                        ),
                                      ],
                                      const SizedBox(height: 10),
                                      FilledButton.icon(
                                        key: const Key('pair-this-device'),
                                        onPressed:
                                            _pairing ? null : _pairThisDevice,
                                        icon:
                                            _pairing
                                                ? const SizedBox.square(
                                                  dimension: 16,
                                                  child:
                                                      CircularProgressIndicator(
                                                        strokeWidth: 2,
                                                      ),
                                                )
                                                : const Icon(
                                                  Icons.link_rounded,
                                                ),
                                        label: const Text('檢查並配對'),
                                      ),
                                    ],
                                  ),
                                ),
                                const SizedBox(height: 20),
                              ],
                              if (widget.voiceSupported) ...[
                                _SettingsSection(
                                  title: '語音輸入',
                                  action: IconButton(
                                    key: const Key('refresh-voice-settings'),
                                    tooltip: '重新整理語音設定',
                                    onPressed:
                                        _voiceSettingsLoading ||
                                                _voiceModelOperation
                                            ? null
                                            : _loadVoiceSettings,
                                    icon:
                                        _voiceSettingsLoading
                                            ? const SizedBox.square(
                                              dimension: 18,
                                              child: CircularProgressIndicator(
                                                strokeWidth: 2,
                                              ),
                                            )
                                            : const Icon(Icons.refresh_rounded),
                                  ),
                                  child: _VoiceSettingsPanel(
                                    modelStatus: _voiceModelStatus,
                                    catalog: _voiceCatalog,
                                    selection: _voiceSelection,
                                    loading: _voiceSettingsLoading,
                                    modelOperation: _voiceModelOperation,
                                    error: _voiceSettingsError,
                                    onSelectLocal: _selectLocalVoiceEngine,
                                    onSelectRemote: _selectRemoteVoiceEngine,
                                    onInstallModel: _confirmInstallVoiceModel,
                                    onDeleteModel: _confirmDeleteVoiceModel,
                                  ),
                                ),
                                const SizedBox(height: 20),
                              ],
                              if (widget.notificationSettingsPanel != null) ...[
                                _SettingsSection(
                                  title: '通知',
                                  action: const SizedBox.shrink(),
                                  child: widget.notificationSettingsPanel!,
                                ),
                                const SizedBox(height: 20),
                              ],
                              _SettingsSection(
                                title: '伺服器',
                                action: IconButton(
                                  tooltip: '重新整理伺服器狀態',
                                  onPressed: _loadDiagnostics,
                                  icon: const Icon(Icons.refresh_rounded),
                                ),
                                child:
                                    _readiness == null && _diagnostics == null
                                        ? _SettingsLoadState(
                                          error:
                                              _readinessError ??
                                              _diagnosticsError,
                                          onRetry: _loadDiagnostics,
                                        )
                                        : Column(
                                          crossAxisAlignment:
                                              CrossAxisAlignment.stretch,
                                          children: [
                                            if (_readinessError != null) ...[
                                              _DriveInlineError(
                                                message:
                                                    '${_readinessError!} '
                                                    '以下為上次成功取得的就緒狀態。',
                                                onRetry: _loadDiagnostics,
                                              ),
                                              const SizedBox(height: 8),
                                            ],
                                            if (_diagnosticsError != null) ...[
                                              _DriveInlineError(
                                                message:
                                                    '${_diagnosticsError!} '
                                                    '以下為上次成功取得的佇列狀態。',
                                                onRetry: _loadDiagnostics,
                                              ),
                                              const SizedBox(height: 8),
                                            ],
                                            _DiagnosticsCard(
                                              readiness: _readiness,
                                              diagnostics: _diagnostics,
                                            ),
                                          ],
                                        ),
                              ),
                              const SizedBox(height: 20),
                              _SettingsSection(
                                title: '已配對裝置',
                                action: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    IconButton(
                                      key: const Key('create-pairing-code'),
                                      tooltip: '配對新裝置',
                                      onPressed:
                                          _creatingPairingCode
                                              ? null
                                              : _createPairingCode,
                                      icon:
                                          _creatingPairingCode
                                              ? const SizedBox.square(
                                                dimension: 18,
                                                child:
                                                    CircularProgressIndicator(
                                                      strokeWidth: 2,
                                                    ),
                                              )
                                              : const Icon(
                                                Icons.add_link_rounded,
                                              ),
                                    ),
                                    IconButton(
                                      tooltip: '重新整理裝置',
                                      onPressed: _loadDevices,
                                      icon: const Icon(Icons.refresh_rounded),
                                    ),
                                  ],
                                ),
                                child:
                                    _devices == null
                                        ? _SettingsLoadState(
                                          error: _devicesError,
                                          onRetry: _loadDevices,
                                        )
                                        : Column(
                                          children: [
                                            if (_devicesError != null)
                                              _DriveInlineError(
                                                message: _devicesError!,
                                                onRetry: _loadDevices,
                                              ),
                                            if (_devices!.isEmpty)
                                              const _DrawerEmptyState(
                                                text: '沒有其他有效裝置。',
                                              )
                                            else
                                              for (final device in _devices!)
                                                _DeviceTile(
                                                  device: device,
                                                  isCurrent:
                                                      device.id ==
                                                      _currentDeviceId,
                                                  revoking:
                                                      _revokingDeviceId ==
                                                      device.id,
                                                  onRevoke:
                                                      () =>
                                                          _revokeDevice(device),
                                                ),
                                          ],
                                        ),
                              ),
                              const SizedBox(height: 28),
                              Text(
                                '目前裝置',
                                style: Theme.of(context).textTheme.titleMedium
                                    ?.copyWith(fontWeight: FontWeight.w600),
                              ),
                              const SizedBox(height: 8),
                              OutlinedButton.icon(
                                key: const Key('logout-device'),
                                onPressed: _loggingOut ? null : _logout,
                                style: OutlinedButton.styleFrom(
                                  foregroundColor:
                                      Theme.of(context).colorScheme.error,
                                ),
                                icon:
                                    _loggingOut
                                        ? const SizedBox.square(
                                          dimension: 16,
                                          child: CircularProgressIndicator(
                                            strokeWidth: 2,
                                          ),
                                        )
                                        : const Icon(Icons.logout_rounded),
                                label: const Text('登出這台裝置'),
                              ),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
        ),
      ),
    );
  }
}

class _ThemeModeSettings extends StatelessWidget {
  const _ThemeModeSettings({
    required this.mode,
    required this.saving,
    required this.error,
    required this.onChanged,
  });

  final ThemeMode mode;
  final bool saving;
  final String? error;
  final ValueChanged<ThemeMode> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          '預設跟隨裝置；你的選擇只保存在這台裝置。',
          style: Theme.of(
            context,
          ).textTheme.bodySmall?.copyWith(color: palette.muted),
        ),
        const SizedBox(height: 10),
        Semantics(
          container: true,
          label: '顯示主題',
          value: _themeModeLabel(mode),
          child: SegmentedButton<ThemeMode>(
            key: const Key('theme-mode-chooser'),
            expandedInsets: EdgeInsets.zero,
            segments: const [
              ButtonSegment(
                value: ThemeMode.system,
                icon: Icon(Icons.brightness_auto_rounded, size: 18),
                label: Text('系統', key: Key('theme-mode-system')),
              ),
              ButtonSegment(
                value: ThemeMode.light,
                icon: Icon(Icons.light_mode_outlined, size: 18),
                label: Text('淺色', key: Key('theme-mode-light')),
              ),
              ButtonSegment(
                value: ThemeMode.dark,
                icon: Icon(Icons.dark_mode_outlined, size: 18),
                label: Text('深色', key: Key('theme-mode-dark')),
              ),
            ],
            selected: {mode},
            showSelectedIcon: true,
            onSelectionChanged:
                saving ? null : (selection) => onChanged(selection.single),
            style: ButtonStyle(
              minimumSize: const WidgetStatePropertyAll(Size(0, 48)),
              textStyle: WidgetStatePropertyAll(
                Theme.of(context).textTheme.labelLarge,
              ),
            ),
          ),
        ),
        if (error != null) ...[
          const SizedBox(height: 8),
          Text(
            error!,
            key: const Key('theme-mode-error'),
            style: Theme.of(context).textTheme.bodySmall?.copyWith(
              color: Theme.of(context).colorScheme.error,
            ),
          ),
        ],
      ],
    );
  }
}

String _themeModeLabel(ThemeMode mode) => switch (mode) {
  ThemeMode.system => '跟隨系統',
  ThemeMode.light => '淺色',
  ThemeMode.dark => '深色',
};

class _PairingTargetRow extends StatelessWidget {
  const _PairingTargetRow({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 76,
            child: Text(label, style: Theme.of(context).textTheme.bodySmall),
          ),
          Expanded(
            child: SelectableText(
              value,
              style: Theme.of(
                context,
              ).textTheme.bodyMedium?.copyWith(fontWeight: FontWeight.w600),
            ),
          ),
        ],
      ),
    );
  }
}

class _SettingsSection extends StatelessWidget {
  const _SettingsSection({
    required this.title,
    required this.action,
    required this.child,
  });

  final String title;
  final Widget action;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                title,
                style: Theme.of(
                  context,
                ).textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
              ),
            ),
            action,
          ],
        ),
        const SizedBox(height: 6),
        child,
      ],
    );
  }
}

class _SettingsLoadState extends StatelessWidget {
  const _SettingsLoadState({required this.error, required this.onRetry});

  final String? error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    if (error != null) {
      return _DrawerErrorState(error: error!, onRetry: onRetry);
    }
    return const _DrawerLoadingState(label: '載入中…');
  }
}

class _DiagnosticsCard extends StatelessWidget {
  const _DiagnosticsCard({required this.readiness, required this.diagnostics});

  final ServerReadiness? readiness;
  final ServerDiagnostics? diagnostics;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final isReady = readiness?.ready;
    final role = diagnostics?.role ?? '—';
    return Container(
      key: const Key('server-diagnostics'),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        border: Border.all(color: palette.outline),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                isReady == true
                    ? Icons.check_circle_outline_rounded
                    : isReady == false
                    ? Icons.warning_amber_rounded
                    : Icons.help_outline_rounded,
                color:
                    isReady == true
                        ? palette.miku
                        : isReady == false
                        ? Theme.of(context).colorScheme.error
                        : palette.muted,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  isReady == true
                      ? '伺服器已就緒'
                      : isReady == false
                      ? '伺服器尚未就緒'
                      : '就緒狀態未知',
                  style: Theme.of(
                    context,
                  ).textTheme.labelLarge?.copyWith(fontWeight: FontWeight.w600),
                ),
              ),
              Text(
                role,
                style: Theme.of(
                  context,
                ).textTheme.labelMedium?.copyWith(color: palette.muted),
              ),
            ],
          ),
          if (readiness != null && readiness!.detail.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(
              _friendlyReadinessDetail(readiness!),
              key: const Key('server-readiness-detail'),
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
          ],
          if (diagnostics != null) ...[
            const SizedBox(height: 8),
            SelectableText(
              diagnostics!.baseUrl,
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                fontFamily: 'monospace',
                color: palette.muted,
              ),
            ),
            const SizedBox(height: 12),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                _MetricChip(label: 'Turns', value: diagnostics!.turnQueueDepth),
                _MetricChip(
                  label: 'Dreams',
                  value: diagnostics!.dreamQueueDepth,
                ),
                _MetricChip(
                  label: 'Scheduler',
                  value: diagnostics!.schedulerQueueDepth,
                ),
                _MetricChip(
                  label: 'Approvals',
                  value: diagnostics!.pendingApprovals,
                ),
                if (diagnostics!.pushQueueDepth != null)
                  _MetricChip(
                    label: 'Push',
                    value: diagnostics!.pushQueueDepth!,
                  ),
              ],
            ),
          ] else ...[
            const SizedBox(height: 10),
            Text(
              '佇列深度目前不可用。',
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
          ],
        ],
      ),
    );
  }
}

String _friendlyReadinessDetail(ServerReadiness readiness) {
  final runtime = readiness.runtime;
  if (runtime.shuttingDown || readiness.status == 'draining') {
    return '伺服器正在排空既有工作，暫不接收新工作。';
  }
  if (runtime.postgres && !runtime.migrationsApplied) {
    return '資料庫遷移尚未完成，暫不接收新工作。';
  }
  if (runtime.workersEnabled && !runtime.postgres) {
    return '工作執行程式需要 Postgres，目前設定不完整。';
  }
  final memory = readiness.memory;
  if (memory != null && !memory.durableWritesReady) {
    final reason = memory.schema.reason;
    return reason == null || reason.isEmpty
        ? '持久記憶目前無法寫入。'
        : '持久記憶目前無法寫入：$reason';
  }
  if (readiness.ready) {
    if (memory == null) return '執行環境已可接收工作。';
    if (memory.denseRetrievalReady) {
      return '執行環境、持久記憶與語意檢索皆已就緒。';
    }
    return '執行環境與持久記憶已就緒；語意檢索目前使用降級路徑。';
  }
  return '伺服器回報狀態：${readiness.status.isEmpty ? '未知' : readiness.status}';
}

class _MetricChip extends StatelessWidget {
  const _MetricChip({required this.label, required this.value});

  final String label;
  final int value;

  @override
  Widget build(BuildContext context) {
    return Chip(
      label: Text('$label · $value'),
      visualDensity: VisualDensity.compact,
    );
  }
}

class _DeviceTile extends StatelessWidget {
  const _DeviceTile({
    required this.device,
    required this.isCurrent,
    required this.revoking,
    required this.onRevoke,
  });

  final AuthDevice device;
  final bool isCurrent;
  final bool revoking;
  final VoidCallback onRevoke;

  @override
  Widget build(BuildContext context) {
    return ListTile(
      key: Key('auth-device-${device.id}'),
      contentPadding: const EdgeInsets.symmetric(horizontal: 4),
      leading: Icon(_deviceIcon(device.platform)),
      title: Text(device.name),
      subtitle: Text(
        device.isActive
            ? '${device.platform} · ${_friendlyTimestamp(device.lastSeenAt)}'
            : '${device.platform} · 已於 ${_friendlyTimestamp(device.revokedAt!)} 撤銷',
      ),
      trailing:
          revoking
              ? const SizedBox.square(
                dimension: 20,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
              : !device.isActive
              ? const Chip(
                label: Text('已撤銷'),
                visualDensity: VisualDensity.compact,
              )
              : isCurrent
              ? Semantics(
                label: '目前使用中的裝置',
                child: const Chip(
                  key: Key('current-auth-device'),
                  label: Text('這台裝置'),
                  visualDensity: VisualDensity.compact,
                ),
              )
              : IconButton(
                tooltip: '撤銷 ${device.name}',
                onPressed: onRevoke,
                icon: const Icon(Icons.link_off_rounded),
              ),
    );
  }
}

IconData _deviceIcon(String platform) {
  return switch (platform.toLowerCase()) {
    'android' || 'ios' => Icons.smartphone_rounded,
    'web' => Icons.language_rounded,
    _ => Icons.devices_other_rounded,
  };
}

String _friendlyTimestamp(String value) {
  final parsed = DateTime.tryParse(value)?.toLocal();
  if (parsed == null) return '最近使用時間未知';
  final month = parsed.month.toString().padLeft(2, '0');
  final day = parsed.day.toString().padLeft(2, '0');
  final hour = parsed.hour.toString().padLeft(2, '0');
  final minute = parsed.minute.toString().padLeft(2, '0');
  return '$month/$day $hour:$minute';
}
