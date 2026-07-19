part of 'main.dart';

class MikuHomePage extends StatefulWidget {
  const MikuHomePage({
    super.key,
    required this.client,
    required this.notifications,
    required this.shareImports,
    required this.voiceCapture,
    this.localAsrWorkers,
    this.localAsrModels,
    this.voiceInferenceTimeout = const Duration(seconds: 45),
  });

  final MikuSessionClient client;
  final MikuNotificationService notifications;
  final MikuShareImportService shareImports;
  final MikuVoiceCaptureService voiceCapture;
  final LocalAsrWorkerFactory? localAsrWorkers;
  final LocalAsrModelManager? localAsrModels;
  final Duration voiceInferenceTimeout;

  @override
  State<MikuHomePage> createState() => _MikuHomePageState();
}

class _MikuHomePageState extends State<MikuHomePage>
    with SingleTickerProviderStateMixin, WidgetsBindingObserver {
  static const _languagePreferenceKey = 'tempest_miku.ui.language.v1';

  final _inputCtrl = TextEditingController();
  final _scrollCtrl = ScrollController();
  final List<ApprovalPrompt> _approvals = [];
  final List<MemoryWriteProposal> _memoryProposals = [];
  final List<String> _nextActions = [];
  final List<_ConversationRound> _rounds = [];
  final List<_Mode> _modes = [];
  DriveFeed? _driveFeed;

  Future<void>? _sessionFuture;
  StreamSubscription<MikuEvent>? _sub;
  StreamSubscription<ApprovalNotificationAction>? _notificationActionSub;
  StreamSubscription<NotificationRouteAction>? _notificationRouteSub;
  StreamSubscription<UnifiedPushEvent>? _unifiedPushSub;
  StreamSubscription<SharedContent>? _shareImportSub;
  final List<ApprovalNotificationAction> _pendingNotificationActions = [];
  final List<NotificationRouteAction> _pendingNotificationRoutes = [];
  final List<SharedContent> _pendingShareImports = [];
  final List<String> _recentQuickCaptureIds = [];
  ValueNotifier<SharedContent>? _activeShareImport;
  bool _processingNotificationActions = false;
  bool _processingNotificationRoutes = false;
  bool _processingShareImports = false;
  bool _voiceRecording = false;
  bool _voiceProcessing = false;
  bool _voicePermissionPending = false;
  bool _voiceModelOperation = false;
  LocalAsrModelStatus? _voiceModelStatus;
  VoiceAsrEngineCatalog _voiceAsrCatalog = VoiceAsrEngineCatalog.localOnly();
  VoiceAsrEngineKind _voiceAsrSelection = VoiceAsrEngineKind.local;
  VoiceAsrEngineKind? _activeVoiceAsrSelection;
  bool _voiceAsrCatalogLoading = false;
  String? _voiceCaptureId;
  int _voiceOperationEpoch = 0;
  Timer? _voiceLimitTimer;
  bool _sessionBootComplete = false;
  String? _sessionId;
  String? _lastEventId;
  String _modeId = '';
  String _defaultModeId = '';
  String _status = 'idle';
  String _projectStatus = '';
  String _driveError = '';
  bool _driveLoading = false;
  bool _modeLocked = false;
  bool _canSend = false;
  bool _isSending = false;
  bool _disconnecting = false;
  bool _needsPairing = false;
  bool _followLatest = true;
  bool _showJumpToLatest = false;
  bool _scrollFrameScheduled = false;
  String _sendError = '';
  String? _pendingMessageId;
  String? _pendingMessageText;
  _ConversationRound? _pendingOptimisticRound;
  int _sessionHistoryRevision = 0;
  int _sessionNavigationEpoch = 0;
  int _serverAuthorityEpoch = 0;
  int _sendEpoch = 0;
  _UiLanguage _language = _UiLanguage.en;
  AppLifecycleState _appLifecycle = AppLifecycleState.resumed;

  late final AnimationController _dotAnim;
  LocalAsrTranscriber? _voiceTranscriber;
  late final Future<VoiceAppBuildFingerprint?> _voiceBuildFingerprint;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _dotAnim = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    )..repeat();
    _voiceTranscriber =
        widget.localAsrWorkers == null
            ? null
            : LocalAsrTranscriber(
              workers: widget.localAsrWorkers!,
              timeout: widget.voiceInferenceTimeout,
            );
    _voiceBuildFingerprint = _inspectVoiceBuild();
    if (widget.localAsrWorkers != null) {
      _voiceModelStatus = const LocalAsrModelStatus(
        state: LocalAsrModelState.ready,
        reason: 'injected worker factory',
        encoder: 'injected',
        decoder: 'injected',
        tokens: 'injected',
      );
    } else if (widget.localAsrModels?.isSupported ?? false) {
      unawaited(_refreshVoiceModel());
    }
    _scrollCtrl.addListener(_handleThreadScroll);
    unawaited(_loadUiPreferences());
    unawaited(widget.notifications.initialize());
    _notificationActionSub = widget.notifications.actions.listen(
      _enqueueNotificationAction,
    );
    final actionableNotifications = _actionableNotifications;
    if (actionableNotifications != null) {
      _notificationRouteSub = actionableNotifications.routes.listen(
        _enqueueNotificationRoute,
      );
    }
    final pushNotifications = _unifiedPushNotifications;
    if (pushNotifications != null) {
      _unifiedPushSub = pushNotifications.pushEvents.listen(
        _handleUnifiedPushEvent,
      );
      unawaited(_initializeUnifiedPush(pushNotifications));
    }
    if (widget.shareImports.isSupported) {
      _shareImportSub = widget.shareImports.imports.listen(
        _enqueueShareImport,
        onError: (_) {},
      );
    }
    if (widget.voiceCapture.isSupported) {
      unawaited(widget.voiceCapture.recoverOrphans().catchError((_) => 0));
    }
    unawaited(_boot());
  }

  Future<VoiceAppBuildFingerprint?> _inspectVoiceBuild() async {
    if (!widget.voiceCapture.isSupported) return null;
    try {
      return await widget.voiceCapture.inspectBuild().timeout(
        const Duration(seconds: 5),
      );
    } catch (_) {
      // Build identity is diagnostic metadata. Its absence must never block
      // recording, local inference, review, or explicit send.
      return null;
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _inputCtrl.dispose();
    _scrollCtrl.dispose();
    _sub?.cancel();
    _notificationActionSub?.cancel();
    _notificationRouteSub?.cancel();
    _unifiedPushSub?.cancel();
    _shareImportSub?.cancel();
    _voiceLimitTimer?.cancel();
    _voiceOperationEpoch += 1;
    unawaited(
      widget.voiceCapture.cancel(_voiceCaptureId).catchError((_) => false),
    );
    unawaited(widget.client.cancelVoiceAsrTranscription());
    final voiceTranscriber = _voiceTranscriber;
    if (voiceTranscriber != null) unawaited(voiceTranscriber.cancel());
    _dotAnim.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _appLifecycle = state;
    if (state != AppLifecycleState.resumed &&
        (_voiceRecording || (_voiceProcessing && !_voicePermissionPending))) {
      unawaited(_cancelVoiceCapture());
    }
  }

  _Mode get _mode => _findMode(_modeId, _modes);
  _Tok get _tok =>
      Theme.of(context).brightness == Brightness.dark ? _Tok.dark : _Tok.light;
  Color get _accent => _tok.accentSoft;
  _UiCopy get _copy => _UiCopy(_language);
  ServerTargetClient? get _serverTargetClient =>
      widget.client is ServerTargetClient
          ? widget.client as ServerTargetClient
          : null;
  PushRegistrationClient? get _pushRegistrationClient =>
      widget.client is PushRegistrationClient
          ? widget.client as PushRegistrationClient
          : null;
  UnifiedPushNotificationService? get _unifiedPushNotifications =>
      widget.notifications is UnifiedPushNotificationService
          ? widget.notifications as UnifiedPushNotificationService
          : null;
  ActionableNotificationService? get _actionableNotifications =>
      widget.notifications is ActionableNotificationService
          ? widget.notifications as ActionableNotificationService
          : null;
  NotificationReplyAuthorityClient? get _notificationReplyAuthorityClient =>
      widget.client is NotificationReplyAuthorityClient
          ? widget.client as NotificationReplyAuthorityClient
          : null;
  bool get _sessionEnded => _status == 'ended';
  VoiceAsrEngine? get _selfHostedVoiceAsr => _voiceAsrCatalog.selfHosted;
  bool get _selfHostedVoiceAsrAvailable =>
      _selfHostedVoiceAsr?.available == true;
  bool get _selectedVoiceAsrReady => switch (_voiceAsrSelection) {
    VoiceAsrEngineKind.local => _voiceTranscriber != null,
    VoiceAsrEngineKind.remote => _selfHostedVoiceAsrAvailable,
  };
  String get _selectedVoiceAsrSummary => switch (_voiceAsrSelection) {
    VoiceAsrEngineKind.local => _copy.pick(
      'On-device · audio stays here',
      '本機 · 音訊留在裝置上',
    ),
    VoiceAsrEngineKind.remote when _voiceAsrCatalogLoading => _copy.pick(
      'Checking home service…',
      '正在檢查家用服務…',
    ),
    VoiceAsrEngineKind.remote when !_selfHostedVoiceAsrAvailable => _copy.pick(
      'Home remote · unavailable',
      '家用遠端 · 目前無法使用',
    ),
    VoiceAsrEngineKind.remote => _copy.pick(
      'Home remote · ${_selfHostedVoiceAsr?.modelId ?? 'self-hosted'}',
      '家用遠端 · ${_selfHostedVoiceAsr?.modelId ?? '自架'}',
    ),
  };

  void _handleThreadScroll() {
    if (!_scrollCtrl.hasClients) return;
    final distance =
        _scrollCtrl.position.maxScrollExtent - _scrollCtrl.position.pixels;
    final shouldFollow = distance < 96;
    final shouldShow = !shouldFollow && distance > 160;
    if (shouldFollow == _followLatest && shouldShow == _showJumpToLatest) {
      return;
    }
    setState(() {
      _followLatest = shouldFollow;
      _showJumpToLatest = shouldShow;
    });
  }

  Future<void> _loadUiPreferences() async {
    try {
      final preferences = await SharedPreferences.getInstance();
      final language = switch (preferences.getString(_languagePreferenceKey)) {
        'zh' => _UiLanguage.zh,
        _ => _UiLanguage.en,
      };
      if (mounted && language != _language) {
        setState(() => _language = language);
      }
    } catch (_) {
      // The UI remains fully usable when local preference storage is absent.
    }
  }

  Future<void> _toggleLanguage() async {
    final language =
        _language == _UiLanguage.en ? _UiLanguage.zh : _UiLanguage.en;
    setState(() => _language = language);
    try {
      final preferences = await SharedPreferences.getInstance();
      await preferences.setString(
        _languagePreferenceKey,
        language == _UiLanguage.zh ? 'zh' : 'en',
      );
    } catch (_) {
      // Keep the in-memory selection even when persistence is unavailable.
    }
  }

  Future<void> _boot() async {
    await _ensureSession();
    await _refreshVoiceAsrEngines();
    _sessionBootComplete = true;
    await _drainNotificationActions();
    await _drainNotificationRoutes();
    await _drainShareImports();
  }

  void _enqueueShareImport(SharedContent content) {
    if (content.source == SharedContentSource.quickCapture ||
        content.source == SharedContentSource.voice) {
      final eventId = content.eventId;
      if (eventId == null || _recentQuickCaptureIds.contains(eventId)) return;
      _recentQuickCaptureIds.add(eventId);
      if (_recentQuickCaptureIds.length > 64) {
        _recentQuickCaptureIds.removeAt(0);
      }
      if (content.source == SharedContentSource.quickCapture) {
        final active = _activeShareImport;
        if (active?.value.source == SharedContentSource.quickCapture) {
          active!.value = content;
          return;
        }
        _pendingShareImports.removeWhere(
          (pending) => pending.source == SharedContentSource.quickCapture,
        );
      }
    }
    _pendingShareImports.add(content);
    if (_sessionBootComplete) unawaited(_drainShareImports());
  }

  Future<void> _drainShareImports() async {
    if (_processingShareImports || !_sessionBootComplete) return;
    _processingShareImports = true;
    try {
      while (_pendingShareImports.isNotEmpty && mounted) {
        final content = _pendingShareImports.removeAt(0);
        await _reviewShareImport(content);
      }
    } finally {
      _processingShareImports = false;
    }
  }

  Future<void> _reviewShareImport(SharedContent content) async {
    if (!mounted) return;
    final contentListenable = ValueNotifier(content);
    _activeShareImport = contentListenable;
    late final _ShareImportDecision? decision;
    try {
      decision = await showModalBottomSheet<_ShareImportDecision>(
        context: context,
        showDragHandle: true,
        backgroundColor: _tok.surface,
        isScrollControlled: true,
        useSafeArea: true,
        shape: const RoundedRectangleBorder(
          borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
        ),
        builder:
            (sheetContext) => _ShareImportSheet(
              contentListenable: contentListenable,
              currentSessionAvailable: _sessionId != null && !_sessionEnded,
              tok: _tok,
              copy: _copy,
            ),
      );
    } finally {
      if (identical(_activeShareImport, contentListenable)) {
        _activeShareImport = null;
      }
      contentListenable.dispose();
    }
    if (decision == null || !mounted) return;
    if (decision.destination == _ShareDestination.newSession) {
      await _startNewSession(initialMessage: decision.text);
      return;
    }
    try {
      await _sendText(decision.text);
    } catch (err) {
      _showSnack(_copy.shareSendFailed(err));
    }
  }

  Future<void> _startVoiceCapture() async {
    final selection = _voiceAsrSelection;
    if (!widget.voiceCapture.isSupported || !_selectedVoiceAsrReady) {
      _showSnack(_copy.voiceEngineUnavailable);
      return;
    }
    if (_disconnecting ||
        _voiceRecording ||
        _voiceProcessing ||
        _appLifecycle != AppLifecycleState.resumed) {
      return;
    }
    final epoch = ++_voiceOperationEpoch;
    setState(() => _voiceProcessing = true);
    _voicePermissionPending = true;
    try {
      final permitted = await widget.voiceCapture.requestPermission();
      if (!mounted || epoch != _voiceOperationEpoch) return;
      _voicePermissionPending = false;
      if (!permitted) {
        _showSnack(_copy.voicePermissionDenied);
        return;
      }
      if (_appLifecycle != AppLifecycleState.resumed) return;
      final captureId = _newVoiceCaptureId();
      await widget.voiceCapture.start(captureId);
      if (!mounted || epoch != _voiceOperationEpoch) {
        await widget.voiceCapture.cancel(captureId);
        return;
      }
      setState(() {
        _voiceCaptureId = captureId;
        _activeVoiceAsrSelection = selection;
        _voiceRecording = true;
      });
      _voiceLimitTimer?.cancel();
      final durationSeconds =
          selection == VoiceAsrEngineKind.remote
              ? _selfHostedVoiceAsr?.maxDurationSeconds ??
                  localAsrMaxDurationSeconds
              : localAsrMaxDurationSeconds;
      _voiceLimitTimer = Timer(Duration(seconds: durationSeconds), () {
        if (mounted && _voiceCaptureId == captureId && _voiceRecording) {
          unawaited(_stopVoiceCapture());
        }
      });
    } catch (error) {
      if (mounted && epoch == _voiceOperationEpoch) {
        _showSnack(_copy.voiceCaptureFailed(error));
      }
    } finally {
      if (mounted && epoch == _voiceOperationEpoch) {
        _voicePermissionPending = false;
        setState(() => _voiceProcessing = false);
      }
    }
  }

  Future<VoiceAsrEngineCatalog> _refreshVoiceAsrEngines() async {
    final authorityEpoch = _serverAuthorityEpoch;
    if (mounted) setState(() => _voiceAsrCatalogLoading = true);
    VoiceAsrEngineCatalog catalog;
    try {
      catalog = await widget.client.voiceAsrEngines();
    } catch (_) {
      catalog = VoiceAsrEngineCatalog.localOnly();
    }
    if (mounted && authorityEpoch == _serverAuthorityEpoch) {
      setState(() {
        _voiceAsrCatalog = catalog;
        _voiceAsrCatalogLoading = false;
      });
    }
    return catalog;
  }

  Future<void> _showVoiceAsrDialog() async {
    if (_disconnecting || _voiceRecording || _voiceProcessing) {
      _showSnack(
        _copy.pick(
          'Finish the current recording or server change first.',
          '請先完成目前的錄音或伺服器切換。',
        ),
      );
      return;
    }
    final authorityEpoch = _serverAuthorityEpoch;
    await _refreshVoiceAsrEngines();
    if (!mounted ||
        _disconnecting ||
        authorityEpoch != _serverAuthorityEpoch) {
      return;
    }
    final remote = _selfHostedVoiceAsr;
    final remoteAvailable = remote?.available == true;
    await showDialog<void>(
      context: context,
      builder:
          (dialogContext) => AlertDialog(
            title: Text(_copy.pick('Voice recognition', '語音辨識方式')),
            content: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ListTile(
                    key: const ValueKey('selectLocalVoiceAsr'),
                    contentPadding: EdgeInsets.zero,
                    leading: const Icon(Icons.phone_android_rounded),
                    title: Text(_copy.pick('On-device', '本機')),
                    subtitle: Text(
                      _copy.pick(
                        'Audio stays on this device. Requires the verified local model.',
                        '音訊留在這台裝置；需要已驗證的本機模型。',
                      ),
                    ),
                    trailing:
                        _voiceAsrSelection == VoiceAsrEngineKind.local
                            ? const Icon(Icons.check_circle_rounded)
                            : null,
                    onTap: () {
                      setState(
                        () => _voiceAsrSelection = VoiceAsrEngineKind.local,
                      );
                      Navigator.pop(dialogContext);
                    },
                  ),
                  const Divider(),
                  ListTile(
                    key: const ValueKey('selectSelfHostedVoiceAsr'),
                    enabled: remoteAvailable,
                    contentPadding: EdgeInsets.zero,
                    leading: const Icon(Icons.home_work_outlined),
                    title: Text(
                      _copy.pick('Home remote (self-hosted)', '家用遠端（自架）'),
                    ),
                    subtitle: Text(
                      remoteAvailable
                          ? _copy.pick(
                            'Configured · ${remote?.modelId ?? remote?.label}',
                            '已設定 · ${remote?.modelId ?? remote?.label}',
                          )
                          : _copy.pick(
                            'Unavailable on the paired server',
                            '配對的 Server 目前未提供',
                          ),
                    ),
                    trailing:
                        _voiceAsrSelection == VoiceAsrEngineKind.remote
                            ? const Icon(Icons.check_circle_rounded)
                            : null,
                    onTap:
                        remoteAvailable
                            ? () async {
                              if (_voiceAsrSelection ==
                                  VoiceAsrEngineKind.remote) {
                                Navigator.pop(dialogContext);
                                return;
                              }
                              final confirmed = await showDialog<bool>(
                                context: dialogContext,
                                barrierDismissible: false,
                                builder:
                                    (confirmContext) => AlertDialog(
                                      title: Text(
                                        _copy.pick(
                                          'Use your home ASR service?',
                                          '使用家裡的 ASR 服務？',
                                        ),
                                      ),
                                      content: Text(
                                        _copy.pick(
                                          'Each recording will leave this device and travel through your paired TempestMiku server to its fixed self-hosted home ASR service. There is no cloud or local fallback. A transcript always opens for review and is never sent to Miku automatically.',
                                          '每段錄音都會離開這台裝置，經由已配對的 TempestMiku Server 傳到它固定設定的家用自架 ASR 服務。不會改送雲端，也不會自動退回本機。轉錄一定先開啟供你確認，絕不會自動傳給 Miku。',
                                        ),
                                      ),
                                      actions: [
                                        TextButton(
                                          onPressed:
                                              () => Navigator.pop(
                                                confirmContext,
                                                false,
                                              ),
                                          child: Text(_copy.cancel),
                                        ),
                                        FilledButton(
                                          key: const ValueKey(
                                            'confirmSelfHostedVoiceAsr',
                                          ),
                                          onPressed:
                                              () => Navigator.pop(
                                                confirmContext,
                                                true,
                                              ),
                                          child: Text(
                                            _copy.pick(
                                              'Use home service',
                                              '使用家用服務',
                                            ),
                                          ),
                                        ),
                                      ],
                                    ),
                              );
                              if (confirmed != true ||
                                  !mounted ||
                                  _disconnecting ||
                                  authorityEpoch != _serverAuthorityEpoch) {
                                return;
                              }
                              setState(
                                () =>
                                    _voiceAsrSelection =
                                        VoiceAsrEngineKind.remote,
                              );
                              if (dialogContext.mounted) {
                                Navigator.pop(dialogContext);
                              }
                            }
                            : null,
                  ),
                ],
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(dialogContext),
                child: Text(_copy.close),
              ),
            ],
          ),
    );
  }

  Future<LocalAsrModelStatus?> _refreshVoiceModel() async {
    final manager = widget.localAsrModels;
    if (manager == null || !manager.isSupported) return null;
    try {
      final status = await manager.inspect();
      if (!mounted) return status;
      await _applyVoiceModelStatus(status);
      return status;
    } catch (error) {
      if (mounted) _showSnack(_copy.voiceCaptureFailed(error));
      return null;
    }
  }

  Future<void> _applyVoiceModelStatus(LocalAsrModelStatus status) async {
    final activeSelection = _activeVoiceAsrSelection ?? _voiceAsrSelection;
    if (!status.ready &&
        widget.localAsrWorkers == null &&
        activeSelection == VoiceAsrEngineKind.local &&
        (_voiceRecording || _voiceProcessing)) {
      // A model transition must never detach the transcriber while leaving its
      // native microphone capture behind. Cancel both sides before publishing
      // the non-ready status.
      final cancelled = await _cancelVoiceCapture();
      if (!cancelled) {
        throw StateError('voice cleanup did not finish');
      }
    }
    final previous = _voiceTranscriber;
    if (previous != null && widget.localAsrWorkers == null) {
      await previous.cancel();
    }
    if (!mounted) return;
    setState(() {
      _voiceModelStatus = status;
      _voiceTranscriber =
          status.ready && widget.localAsrModels != null
              ? LocalAsrTranscriber(
                workers: widget.localAsrModels!,
                timeout: widget.voiceInferenceTimeout,
              )
              : widget.localAsrWorkers == null
              ? null
              : _voiceTranscriber;
    });
  }

  Future<void> _showVoiceModelDialog() async {
    final manager = widget.localAsrModels;
    if (manager == null || !manager.isSupported) {
      _showSnack(_copy.voiceModelUnavailable);
      return;
    }
    var status = await _refreshVoiceModel();
    if (!mounted || status == null) return;
    await showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder:
          (dialogContext) => StatefulBuilder(
            builder: (dialogContext, setDialogState) {
              final stateLabel = switch (status!.state) {
                LocalAsrModelState.ready => _copy.pick(
                  'Installed and verified',
                  '已安裝並驗證',
                ),
                LocalAsrModelState.missing => _copy.pick(
                  'Not installed',
                  '尚未安裝',
                ),
                LocalAsrModelState.corrupt => _copy.pick(
                  'Corrupt — disabled',
                  '檔案毀損，已停用',
                ),
                LocalAsrModelState.unsupported => _copy.pick(
                  'Unsupported',
                  '不支援',
                ),
              };
              Future<void> install() async {
                final confirmed = await showDialog<bool>(
                  context: dialogContext,
                  barrierDismissible: false,
                  builder:
                      (confirmContext) => AlertDialog(
                        title: Text(
                          _copy.pick('Install local voice model?', '安裝本機語音模型？'),
                        ),
                        content: Text(
                          _copy.pick(
                            'This explicit owner action downloads 226 MiB from the commit-pinned csukuangfj model on Hugging Face. It is stored only in Android no-backup app storage under Apache-2.0. Recognition stays offline; transcripts still require review before sending.',
                            '這個明確的擁有者操作會從 Hugging Face 下載固定 commit 的 csukuangfj 模型（226 MiB），依 Apache-2.0 存在 Android 不備份的應用程式私有空間。辨識保持離線，轉錄仍必須確認後才能送出。',
                          ),
                        ),
                        actions: [
                          TextButton(
                            onPressed:
                                () => Navigator.pop(confirmContext, false),
                            child: Text(_copy.cancel),
                          ),
                          FilledButton(
                            key: const ValueKey('confirmVoiceModelInstall'),
                            onPressed:
                                () => Navigator.pop(confirmContext, true),
                            child: Text(
                              _copy.pick('Download and verify', '下載並驗證'),
                            ),
                          ),
                        ],
                      ),
                );
                if (confirmed != true || !mounted) return;
                setDialogState(() => _voiceModelOperation = true);
                try {
                  status = await manager.install();
                  await _applyVoiceModelStatus(status!);
                } catch (error) {
                  if (mounted) _showSnack(_copy.voiceCaptureFailed(error));
                  status = await manager.inspect();
                  await _applyVoiceModelStatus(status!);
                } finally {
                  if (mounted) {
                    setDialogState(() => _voiceModelOperation = false);
                  }
                }
              }

              Future<void> remove() async {
                setDialogState(() => _voiceModelOperation = true);
                try {
                  if (_voiceRecording || _voiceProcessing) {
                    final cancelled = await _cancelVoiceCapture();
                    if (!cancelled) {
                      throw StateError(
                        'voice cleanup must finish before deleting the model',
                      );
                    }
                  } else {
                    await _voiceTranscriber?.cancel();
                  }
                  status = await manager.delete();
                  await _applyVoiceModelStatus(status!);
                } catch (error) {
                  if (mounted) _showSnack(_copy.voiceCaptureFailed(error));
                } finally {
                  if (mounted) {
                    setDialogState(() => _voiceModelOperation = false);
                  }
                }
              }

              return PopScope(
                canPop: !_voiceModelOperation,
                child: AlertDialog(
                  title: Text(_copy.pick('On-device voice model', '裝置端語音模型')),
                  content: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(stateLabel),
                      const SizedBox(height: 8),
                      Text(
                        status!.reason,
                        style: TextStyle(color: _tok.muted, fontSize: 12),
                      ),
                      const SizedBox(height: 12),
                      Text(
                        _copy.pick(
                          'Local only · Traditional Chinese conversion on device · edit and confirm every transcript',
                          '僅限本機 · 在裝置上轉為繁體中文 · 每次轉錄都要編輯並確認',
                        ),
                        style: TextStyle(color: _tok.muted, fontSize: 12),
                      ),
                      if (_voiceModelOperation) ...[
                        const SizedBox(height: 16),
                        const LinearProgressIndicator(),
                      ],
                    ],
                  ),
                  actions: [
                    if (status!.ready ||
                        status!.state == LocalAsrModelState.corrupt)
                      TextButton(
                        key: const ValueKey('deleteVoiceModel'),
                        onPressed: _voiceModelOperation ? null : remove,
                        child: Text(_copy.pick('Delete', '刪除')),
                      ),
                    if (!status!.ready &&
                        status!.state != LocalAsrModelState.unsupported)
                      FilledButton(
                        key: const ValueKey('installVoiceModel'),
                        onPressed: _voiceModelOperation ? null : install,
                        child: Text(_copy.pick('Install', '安裝')),
                      ),
                    TextButton(
                      onPressed:
                          _voiceModelOperation
                              ? null
                              : () => Navigator.pop(dialogContext),
                      child: Text(_copy.close),
                    ),
                  ],
                ),
              );
            },
          ),
    );
  }

  Future<void> _stopVoiceCapture() async {
    final captureId = _voiceCaptureId;
    final selection = _activeVoiceAsrSelection ?? _voiceAsrSelection;
    if (!_voiceRecording || captureId == null) return;
    final epoch = ++_voiceOperationEpoch;
    _voiceLimitTimer?.cancel();
    _voiceLimitTimer = null;
    setState(() {
      _voiceRecording = false;
      _voiceProcessing = true;
      _voiceCaptureId = null;
      _activeVoiceAsrSelection = null;
    });
    CapturedVoicePcm? captured;
    LocalAsrAudio? audio;
    try {
      captured = await widget.voiceCapture.stop(captureId);
      if (!mounted || epoch != _voiceOperationEpoch) return;
      final qualityIssue = captured.diagnostics.qualityIssue;
      if (captured.captureId != captureId) {
        throw const FormatException('voice capture id changed while stopping');
      }
      late final String transcriptText;
      late final VoiceTranscriptProvenance provenance;
      switch (selection) {
        case VoiceAsrEngineKind.local:
          final transcriber = _voiceTranscriber;
          if (transcriber == null) {
            throw StateError('the on-device voice model became unavailable');
          }
          audio = LocalAsrAudio.fromPcm16(
            captured.pcm16,
            sampleRate: captured.sampleRate,
          );
          final transcript = await transcriber.transcribe(audio);
          transcriptText = transcript.text;
          provenance = VoiceTranscriptProvenance.local;
          break;
        case VoiceAsrEngineKind.remote:
          final transcript = await widget.client.transcribeVoicePcm16(
            engineId: selfHostedVoiceAsrEngineId,
            captureId: captureId,
            sampleRate: captured.sampleRate,
            pcm16: captured.pcm16,
          );
          transcriptText = transcript.text;
          provenance = VoiceTranscriptProvenance.selfHosted;
          break;
      }
      if (!mounted || epoch != _voiceOperationEpoch) return;
      final buildFingerprint = await _voiceBuildFingerprint;
      if (!mounted || epoch != _voiceOperationEpoch) return;
      final reviewed = SharedContent.fromEvent({
        'source': 'voice',
        'eventId': captureId,
        'text': transcriptText,
        'voiceTranscriptProvenance':
            provenance == VoiceTranscriptProvenance.selfHosted
                ? 'self_hosted'
                : 'local',
        if (qualityIssue != null) 'voiceQualityIssue': qualityIssue.name,
        'voiceDiagnostics': captured.diagnostics,
        if (buildFingerprint != null) 'voiceBuildFingerprint': buildFingerprint,
      });
      _enqueueShareImport(reviewed);
    } on LocalAsrCancelledException {
      // Explicit cancellation never creates a review or sends a message.
    } on TimeoutException catch (error) {
      if (mounted && epoch == _voiceOperationEpoch) {
        _showSnack(_copy.voiceCaptureFailed(error));
      }
    } on FormatException catch (error) {
      if (mounted && epoch == _voiceOperationEpoch) {
        _showSnack(
          error.message.toString().contains('empty')
              ? _copy.voiceTranscriptEmpty
              : _copy.voiceCaptureFailed(error),
        );
      }
    } catch (error) {
      if (mounted && epoch == _voiceOperationEpoch) {
        _showSnack(_copy.voiceCaptureFailed(error));
      }
    } finally {
      captured?.pcm16.fillRange(0, captured.pcm16.length, 0);
      audio?.samples.fillRange(0, audio.samples.length, 0);
      if (mounted && epoch == _voiceOperationEpoch) {
        setState(() {
          _voiceProcessing = false;
          _activeVoiceAsrSelection = null;
        });
      }
    }
  }

  Future<bool> _cancelVoiceCapture() async {
    final captureId = _voiceCaptureId;
    final epoch = ++_voiceOperationEpoch;
    _voicePermissionPending = false;
    _voiceLimitTimer?.cancel();
    _voiceLimitTimer = null;
    if (mounted) {
      setState(() {
        _voiceRecording = false;
        _voiceProcessing = true;
      });
    }
    final transcriber = _voiceTranscriber;
    try {
      final results = await Future.wait<Object?>([
        widget.voiceCapture.cancel(captureId),
        widget.client.cancelVoiceAsrTranscription(),
        if (transcriber != null)
          transcriber.cancel().then<Object?>((_) => null),
      ]);
      final nativeCancelled = results.first as bool;
      if (captureId != null && !nativeCancelled) {
        throw StateError('native voice recorder cleanup did not finish');
      }
      if (mounted && epoch == _voiceOperationEpoch) {
        setState(() {
          _voiceProcessing = false;
          _voiceCaptureId = null;
          _activeVoiceAsrSelection = null;
        });
      }
      return true;
    } catch (error) {
      // Keep the cancel affordance and capture id visible so a retiring native
      // recorder can be retried. The epoch already prevents any stale
      // transcription result from entering review or send.
      if (mounted && epoch == _voiceOperationEpoch) {
        setState(() {
          _voiceRecording = false;
          _voiceProcessing = true;
          _voiceCaptureId = captureId;
        });
        _showSnack(_copy.voiceCaptureFailed(error));
      }
      return false;
    }
  }

  String _newVoiceCaptureId() {
    final random = math.Random.secure();
    final bytes = List<int>.generate(16, (_) => random.nextInt(256));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    String hex(int value) => value.toRadixString(16).padLeft(2, '0');
    final value = bytes.map(hex).join();
    return '${value.substring(0, 8)}-'
        '${value.substring(8, 12)}-'
        '${value.substring(12, 16)}-'
        '${value.substring(16, 20)}-'
        '${value.substring(20)}';
  }

  Future<bool> _beginServerAuthorityTransition() async {
    if (_disconnecting) return false;
    _serverAuthorityEpoch += 1;
    if (mounted) {
      setState(() => _disconnecting = true);
    } else {
      return false;
    }
    final voiceActive =
        _voiceRecording ||
        _voiceProcessing ||
        _voicePermissionPending ||
        _voiceCaptureId != null ||
        _activeVoiceAsrSelection != null;
    if (!voiceActive) {
      _resetVoiceAsrAuthority();
      return true;
    }
    final cancelled = await _cancelVoiceCapture();
    if (cancelled && mounted) {
      _resetVoiceAsrAuthority();
      return true;
    }
    if (mounted) {
      setState(() {
        _disconnecting = false;
        _voiceAsrCatalogLoading = false;
      });
    }
    return false;
  }

  void _resetVoiceAsrAuthority() {
    if (!mounted) return;
    setState(() {
      _voiceAsrSelection = VoiceAsrEngineKind.local;
      _activeVoiceAsrSelection = null;
      _voiceAsrCatalog = VoiceAsrEngineCatalog.localOnly();
      _voiceAsrCatalogLoading = false;
    });
  }

  void _endServerAuthorityTransition() {
    if (mounted) setState(() => _disconnecting = false);
  }

  Future<bool> _applyPairingLink(String rawLink) async {
    final client = _serverTargetClient;
    if (client == null) return false;
    var authorityTransitionStarted = false;
    try {
      final target = pairingTargetFromLink(rawLink);
      final proposedDeviceName = client.pairingDeviceName();
      if (!mounted) return false;
      final approved = await showDialog<bool>(
        context: context,
        barrierDismissible: false,
        builder:
            (dialogContext) => AlertDialog(
              backgroundColor: _tok.surface,
              title: const Text('Pair with this server?'),
              content: PairingAuthorityDetails(
                target: target,
                deviceName: proposedDeviceName,
              ),
              actions: [
                TextButton(
                  onPressed: () => Navigator.pop(dialogContext, false),
                  child: Text(_copy.cancel),
                ),
                FilledButton(
                  onPressed: () => Navigator.pop(dialogContext, true),
                  child: const Text('Pair securely'),
                ),
              ],
            ),
      );
      if (approved != true) return false;
      if (!await _beginServerAuthorityTransition()) return false;
      authorityTransitionStarted = true;
      await client.pairWithCode(target);
      await _reconnectAfterPair(
        successMessage: _copy.pairedToServer(target.serverBaseUrl),
      );
      try {
        await _requestApprovalNotifications();
      } catch (_) {
        // Notification permission is optional and cannot invalidate a completed
        // authority transition or restore the prior remote-ASR selection.
      }
      final pushNotifications = _unifiedPushNotifications;
      if (pushNotifications != null) {
        await _initializeUnifiedPush(pushNotifications);
      }
      return true;
    } catch (err) {
      if (authorityTransitionStarted) _endServerAuthorityTransition();
      _showSnack(_copy.pairingLinkFailed(err));
      return false;
    }
  }

  void _showSnack(String text) {
    if (!mounted) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
    });
  }

  Future<void> _initializeUnifiedPush(
    UnifiedPushNotificationService notifications,
  ) async {
    try {
      await _syncInlineReplyAuthority();
      final client = _pushRegistrationClient;
      if (client == null || !await client.hasDeviceCredential()) return;
      final registration = await notifications.registerUnifiedPush();
      if (registration != null) {
        await client.registerPush(
          endpoint: registration.endpoint,
          p256dh: registration.p256dh,
          auth: registration.auth,
        );
      }
    } catch (_) {
      // Push is optional and must not turn successful pairing into a failed login.
    }
  }

  Future<void> _syncInlineReplyAuthority() async {
    final notifications = _actionableNotifications;
    if (notifications == null) return;
    final authority =
        await _notificationReplyAuthorityClient?.notificationReplyAuthority();
    await notifications.configureReplyAuthority(
      serverBaseUrl: authority?.serverBaseUrl,
      deviceToken: authority?.deviceToken,
    );
  }

  Future<void> _handleUnifiedPushEvent(UnifiedPushEvent event) async {
    try {
      final client = _pushRegistrationClient;
      if (client == null || !await client.hasDeviceCredential()) return;
      switch (event.type) {
        case UnifiedPushEventType.registration:
          final registration = event.registration;
          if (registration != null) {
            await client.registerPush(
              endpoint: registration.endpoint,
              p256dh: registration.p256dh,
              auth: registration.auth,
            );
          }
          return;
        case UnifiedPushEventType.unregistered:
          await client.unregisterPush();
          return;
        case UnifiedPushEventType.registrationFailed:
          return;
      }
    } catch (_) {
      // The durable registration is retried the next time the app starts.
    }
  }

  // ── Session ────────────────────────────────────────────────────────────────

  int _nextSessionNavigationEpoch() {
    _sendEpoch += 1;
    return ++_sessionNavigationEpoch;
  }

  Future<void> _ensureSession() async {
    if (_sessionId != null) return;
    final pending = _sessionFuture;
    if (pending != null) return pending;
    final navigationEpoch = _nextSessionNavigationEpoch();
    final future = _connectSession(navigationEpoch);
    _sessionFuture = future;
    return future;
  }

  Future<void> _connectSession(int navigationEpoch) async {
    if (mounted && navigationEpoch == _sessionNavigationEpoch) {
      setState(() {
        _status = 'connecting';
        _needsPairing = false;
        _isSending = false;
        _canSend = false;
      });
    }
    try {
      await _loadModes(navigationEpoch: navigationEpoch);
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
      final s = await widget.client.createOrReuseSession();
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
      LoadedSession? loaded;
      try {
        loaded = await widget.client.loadSession(s.id);
      } catch (_) {
        loaded = null;
      }
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
      await _attachSession(
        loaded?.session ?? s,
        messages: loaded?.messages ?? const [],
        pendingEvents: loaded?.pendingEvents ?? const [],
        navigationEpoch: navigationEpoch,
      );
    } catch (err) {
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
      final pairingRequired = _isPairingRequiredError(err);
      setState(() {
        _status = 'offline';
        _needsPairing = pairingRequired;
      });
      if (!pairingRequired) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not connect to tm-server: $err')),
        );
      }
    } finally {
      if (navigationEpoch == _sessionNavigationEpoch) {
        _sessionFuture = null;
      }
    }
  }

  bool _isPairingRequiredError(Object error) {
    if (_serverTargetClient == null) return false;
    final message = error.toString().toLowerCase();
    return message.contains('not securely paired') ||
        message.contains('unauthorized') ||
        message.contains('status 401') ||
        message.contains('http 401');
  }

  Future<void> _attachSession(
    MikuSession session, {
    List<SessionMessage> messages = const [],
    List<MikuEvent> pendingEvents = const [],
    required int navigationEpoch,
  }) async {
    if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
    final previousSub = _sub;
    _sub = null;
    final previousSessionId = _sessionId;
    if (previousSub != null) unawaited(previousSub.cancel());
    final changingSession =
        previousSessionId != null && previousSessionId != session.id;
    if (changingSession) {
      _inputCtrl.clear();
    }
    setState(() {
      _mergeSessionMode(session);
      _sessionId = session.id;
      _sessionHistoryRevision += 1;
      _lastEventId = session.lastEventId;
      _modeId = session.mode.isEmpty ? _defaultModeId : session.mode;
      _modeLocked = session.locked;
      _status = session.status == 'ended' ? 'ended' : 'connected';
      _approvals.clear();
      _memoryProposals.clear();
      _driveFeed = null;
      _driveError = '';
      _driveLoading = false;
      if (changingSession) {
        _isSending = false;
        _pendingMessageId = null;
        _pendingMessageText = null;
        _pendingOptimisticRound = null;
        _sendError = '';
      }
      _rounds
        ..clear()
        ..addAll(_roundsFromMessages(messages));
      for (final event in pendingEvents) {
        _applyEvent(event);
      }
      if (session.status == 'ended') {
        _status = 'ended';
        _canSend = false;
      } else {
        _canSend = _inputCtrl.text.trim().isNotEmpty;
      }
    });
    if (session.status != 'ended') {
      _sub = widget.client
          .events(session.id, lastEventId: _lastEventId)
          .listen(
            (event) => _onEvent(session.id, event),
            onError: (_) {
              if (mounted && _sessionId == session.id && !_sessionEnded) {
                setState(() => _status = 'reconnecting');
              }
            },
          );
    }
    await _loadProject();
    await _loadDriveFeed(silent: true);
    if (mounted &&
        navigationEpoch == _sessionNavigationEpoch &&
        _sessionId == session.id) {
      setState(() {});
      _scrollToBottom(force: true);
    }
  }

  Future<void> _loadModes({int? navigationEpoch}) async {
    final catalog = await widget.client.modeCatalog();
    if (!mounted ||
        (navigationEpoch != null &&
            navigationEpoch != _sessionNavigationEpoch)) {
      return;
    }
    setState(() {
      _defaultModeId = catalog.defaultMode;
      _modes
        ..clear()
        ..addAll(catalog.modes.map(_Mode.fromProfile));
      if (_modes.isEmpty) {
        _modes.add(_Mode.fallback(_defaultModeId));
      }
    });
  }

  void _mergeSessionMode(MikuSession session) {
    if (session.mode.isEmpty || _modes.any((mode) => mode.id == session.mode)) {
      return;
    }
    _modes.add(
      _Mode.fromProfile(
        ModeProfile(
          id: session.mode,
          label: session.label.isEmpty ? session.mode : session.label,
          voiceCap: session.voiceCap.isEmpty ? 'medium' : session.voiceCap,
          defaultScope: session.defaultScope,
          capabilityClass:
              session.defaultScope.startsWith('project:')
                  ? 'engineering'
                  : 'conversation',
          activeSkills: session.activeSkills,
          capabilities: const [],
          description: 'Runtime mode profile from session.',
        ),
      ),
    );
  }

  List<_ConversationRound> _roundsFromMessages(List<SessionMessage> messages) {
    final rounds = <_ConversationRound>[];
    for (final message in messages) {
      if (message.role == 'user') {
        rounds.add(
          _ConversationRound(
            index: rounds.length + 1,
            userText: message.content,
            isStreaming: false,
          ),
        );
        continue;
      }
      if (message.role != 'assistant') continue;
      final round =
          rounds.isNotEmpty && rounds.last.assistantFinalText.isEmpty
              ? rounds.last
              : _ConversationRound(
                index: rounds.length + 1,
                userText: '',
                isStreaming: false,
              );
      if (!rounds.contains(round)) rounds.add(round);
      round.assistantFinalText = message.content;
      round.assistantStreamedText = '';
      round.isStreaming = false;
    }
    return rounds;
  }

  void _onEvent(String sessionId, MikuEvent e) {
    if (_sessionId != sessionId) return;
    _rememberEventCursor(e);
    setState(() {
      _applyEvent(e);
      if (e.type == 'final' || e.type == 'session_end') {
        _sessionHistoryRevision += 1;
      }
    });
    if (_shouldRefreshDriveFeed(e)) {
      unawaited(_loadDriveFeed(silent: true));
    }
    _scrollToBottom();
  }

  Future<void> _requestApprovalNotifications() async {
    if (!widget.notifications.isSupported) return;
    final granted = await widget.notifications.requestPermission();
    if (!granted) {
      _showSnack(
        'Approval alerts are disabled. You can enable notifications in Android settings.',
      );
    }
  }

  void _notifyApprovalIfBackgrounded(ApprovalPrompt approval) {
    if (_appLifecycle == AppLifecycleState.resumed) return;
    final sessionId = _sessionId;
    if (sessionId == null || sessionId.isEmpty) return;
    unawaited(
      widget.notifications.showApproval(
        sessionId: sessionId,
        approvalId: approval.approvalId,
        action: approval.action,
      ),
    );
  }

  void _enqueueNotificationAction(ApprovalNotificationAction action) {
    _pendingNotificationActions.removeWhere(
      (queued) => queued.approvalId == action.approvalId,
    );
    _pendingNotificationActions.add(action);
    unawaited(_drainNotificationActions());
  }

  void _enqueueNotificationRoute(NotificationRouteAction route) {
    _pendingNotificationRoutes.removeWhere(
      (queued) =>
          queued.sessionId == route.sessionId &&
          queued.kind == route.kind &&
          queued.approvalId == route.approvalId,
    );
    _pendingNotificationRoutes.add(route);
    unawaited(_drainNotificationRoutes());
  }

  Future<void> _drainNotificationRoutes() async {
    if (_processingNotificationRoutes || !_sessionBootComplete) return;
    _processingNotificationRoutes = true;
    try {
      while (mounted && _pendingNotificationRoutes.isNotEmpty) {
        final route = _pendingNotificationRoutes.removeAt(0);
        try {
          await _loadModes();
          await _syncNotificationSession(route.sessionId);
        } catch (_) {
          if (mounted) {
            _showSnack('This notification target is no longer available.');
          }
        }
      }
    } finally {
      _processingNotificationRoutes = false;
    }
  }

  Future<void> _drainNotificationActions() async {
    if (_processingNotificationActions || !_sessionBootComplete) return;
    _processingNotificationActions = true;
    try {
      while (mounted && _pendingNotificationActions.isNotEmpty) {
        final action = _pendingNotificationActions.removeAt(0);
        await _handleNotificationAction(action);
      }
    } finally {
      _processingNotificationActions = false;
    }
  }

  Future<void> _handleNotificationAction(
    ApprovalNotificationAction notificationAction,
  ) async {
    try {
      await _loadModes();
      await _syncNotificationSession(notificationAction.sessionId);
      if (!mounted) return;
      ApprovalPrompt? approval;
      for (final candidate in _approvals) {
        if (candidate.approvalId == notificationAction.approvalId) {
          approval = candidate;
          break;
        }
      }
      if (approval == null) {
        await widget.notifications.cancelApproval(
          notificationAction.approvalId,
        );
        _showSnack('This approval was already resolved or has expired.');
        return;
      }
      if (notificationAction.requiresConfirmation) {
        final confirmed = await showDialog<bool>(
          context: context,
          barrierDismissible: false,
          builder:
              (dialogContext) => AlertDialog(
                title: Text(
                  notificationAction.decision == 'approve'
                      ? _copy.approveOnce
                      : _copy.deny,
                ),
                content: Text(
                  approval!.scope.isEmpty
                      ? approval.action
                      : '${approval.action}\n\nScope: ${approval.scope}',
                ),
                actions: [
                  TextButton(
                    onPressed: () => Navigator.pop(dialogContext, false),
                    child: Text(_copy.cancel),
                  ),
                  FilledButton(
                    onPressed: () => Navigator.pop(dialogContext, true),
                    child: Text(
                      notificationAction.decision == 'approve'
                          ? _copy.approveOnce
                          : _copy.deny,
                    ),
                  ),
                ],
              ),
        );
        if (confirmed != true) return;
      }
      await widget.client.resolveApproval(
        notificationAction.sessionId,
        notificationAction.approvalId,
        notificationAction.decision,
      );
      if (!mounted) return;
      setState(
        () => _approvals.removeWhere(
          (candidate) => candidate.approvalId == notificationAction.approvalId,
        ),
      );
      await widget.notifications.cancelApproval(notificationAction.approvalId);
    } catch (error) {
      if (!mounted) return;
      try {
        await _syncNotificationSession(notificationAction.sessionId);
      } catch (_) {}
      if (!mounted) return;
      final stillPending = _approvals.any(
        (approval) => approval.approvalId == notificationAction.approvalId,
      );
      if (!stillPending) {
        await widget.notifications.cancelApproval(
          notificationAction.approvalId,
        );
        _showSnack('This approval was already resolved or has expired.');
      } else {
        _showSnack('Could not resolve approval: $error');
      }
    }
  }

  Future<void> _syncNotificationSession(String sessionId) async {
    if (_disconnecting) return;
    final shouldNavigate = _sessionId != sessionId;
    final navigationEpoch =
        shouldNavigate
            ? _nextSessionNavigationEpoch()
            : _sessionNavigationEpoch;
    final loaded = await widget.client.loadSession(sessionId);
    if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
    if (shouldNavigate) {
      await _attachSession(
        loaded.session,
        messages: loaded.messages,
        pendingEvents: loaded.pendingEvents,
        navigationEpoch: navigationEpoch,
      );
      return;
    }
    if (!mounted) return;
    setState(() {
      _approvals.clear();
      _memoryProposals.clear();
      for (final event in loaded.pendingEvents) {
        _applyEvent(event);
      }
    });
  }

  void _rememberEventCursor(MikuEvent e) {
    final eventId = e.id;
    if (eventId != null &&
        eventId.isNotEmpty &&
        shouldRememberEventId(e.type, e.data)) {
      _lastEventId = eventId;
      final sessionId = _sessionId;
      if (sessionId != null) {
        widget.client.rememberLastEventId(sessionId, eventId);
      }
    }
  }

  void _applyEvent(MikuEvent e) {
    final activity = _activityFromEvent(e);
    if (activity != null) {
      _appendActivity(activity);
    }
    switch (e.type) {
      case 'connection':
        _status = e.data['status'] as String? ?? _status;
      case 'reasoning':
        final delta = e.data['delta'] as String? ?? '';
        if (delta.isNotEmpty) {
          final round = _ensureAssistantRound();
          round.reasoningText += delta;
          round.isStreaming = true;
          round.reasoningExpanded = false;
          _status = 'streaming';
        }
      case 'text':
        final delta = e.data['delta'] as String? ?? '';
        if (delta.isNotEmpty) {
          final round = _ensureAssistantRound();
          round.assistantStreamedText += delta;
          round.isStreaming = true;
          _status = 'streaming';
        }
      case 'final':
        final text = e.data['text'] as String? ?? '';
        final round = _ensureAssistantRound();
        round.assistantFinalText = text;
        round.assistantStreamedText = '';
        round.isStreaming = false;
        _status = 'connected';
        unawaited(_loadProject());
      case 'session_end':
        if (_rounds.isNotEmpty) {
          _rounds.last.isStreaming = false;
        }
        _status = 'ended';
        _canSend = false;
        _approvals.clear();
        _memoryProposals.clear();
      case 'mode':
        final newId = e.data['mode'] as String? ?? _modeId;
        _mergeSessionMode(
          MikuSession(
            id: _sessionId ?? '',
            mode: newId,
            label: e.data['label'] as String? ?? newId,
            voiceCap:
                (e.data['voice_cap'] as String?) ??
                (e.data['voiceCap'] as String?) ??
                'medium',
            defaultScope:
                (e.data['defaultScope'] as String?) ??
                (e.data['default_scope'] as String?) ??
                'global',
            activeSkills:
                ((e.data['activeSkills'] as List?) ?? const [])
                    .map((skill) => skill.toString())
                    .toList(),
          ),
        );
        _modeLocked =
            (e.data['locked'] as bool?) ??
            (e.data['lockSource'] != null || e.data['lock_source'] != null);
        _modeId = newId;
      case 'approval':
        final approval = ApprovalPrompt(
          approvalId: e.data['approvalId'] as String? ?? '',
          action: e.data['action'] as String? ?? 'Approval requested',
          scope: (e.data['scope'] as Map?)?.cast<String, Object?>() ?? const {},
          backend: e.data['backend'] as String? ?? '',
          options:
              ((e.data['options'] as List?) ?? const [])
                  .whereType<Map>()
                  .map(
                    (option) => ApprovalOption(
                      optionId:
                          (option['optionId'] as String?) ??
                          (option['option_id'] as String?) ??
                          '',
                      name: (option['name'] as String?) ?? '',
                      kind: (option['kind'] as String?) ?? '',
                    ),
                  )
                  .where((option) => option.optionId.isNotEmpty)
                  .toList(),
          timeoutMs:
              (e.data['timeoutMs'] as num?)?.toInt() ??
              (e.data['timeout_ms'] as num?)?.toInt(),
        );
        _upsertApproval(approval);
        _notifyApprovalIfBackgrounded(approval);
        final proposal = MemoryWriteProposal.fromApproval(approval);
        if (proposal != null) {
          _upsertMemoryProposal(proposal, onlyIfMissing: true);
        }
      case 'approval_resolved':
        final approvalId = e.data['approvalId'] as String?;
        _approvals.removeWhere((a) => a.approvalId == approvalId);
        if (approvalId != null && approvalId.isNotEmpty) {
          unawaited(widget.notifications.cancelApproval(approvalId));
        }
      case 'write_proposal':
        final proposal = MemoryWriteProposal.fromEvent(e.data);
        if (proposal != null) {
          _upsertMemoryProposal(proposal);
        }
    }
  }

  void _appendActivity(_ActivityItem item) {
    final round = _ensureAssistantRound();
    round.activities.add(item);
    if (round.activities.length > 128) {
      round.activities.removeRange(0, round.activities.length - 128);
    }
    round.isStreaming = true;
    round.activityExpanded = false;
    _status = 'streaming';
  }

  _ConversationRound _ensureAssistantRound() {
    if (_rounds.isNotEmpty && !_rounds.last.isComplete) {
      return _rounds.last;
    }
    final round = _ConversationRound(index: _rounds.length + 1, userText: '');
    _rounds.add(round);
    return round;
  }

  void _upsertApproval(ApprovalPrompt approval) {
    if (approval.approvalId.isEmpty) return;
    _approvals.removeWhere((item) => item.approvalId == approval.approvalId);
    _approvals.add(approval);
  }

  void _upsertMemoryProposal(
    MemoryWriteProposal proposal, {
    bool onlyIfMissing = false,
  }) {
    final index = _memoryProposals.indexWhere(
      (item) => item.proposalId == proposal.proposalId,
    );
    if (!proposal.isPending) {
      if (index != -1) _memoryProposals.removeAt(index);
      return;
    }
    if (index != -1) {
      if (!onlyIfMissing) _memoryProposals[index] = proposal;
      return;
    }
    _memoryProposals.add(proposal);
  }

  void _scrollToBottom({bool force = false, bool animate = false}) {
    if (!force && !_followLatest) {
      if (!_showJumpToLatest && mounted) {
        setState(() => _showJumpToLatest = true);
      }
      return;
    }
    if (_scrollFrameScheduled) return;
    _scrollFrameScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _scrollFrameScheduled = false;
      if (_scrollCtrl.hasClients) {
        final media = MediaQuery.maybeOf(context);
        final reduceMotion =
            media?.disableAnimations == true ||
            media?.accessibleNavigation == true;
        if (animate && !reduceMotion) {
          _scrollCtrl.animateTo(
            _scrollCtrl.position.maxScrollExtent,
            duration: const Duration(milliseconds: 240),
            curve: Curves.easeOut,
          );
        } else {
          _scrollCtrl.jumpTo(_scrollCtrl.position.maxScrollExtent);
        }
        if (mounted && (!_followLatest || _showJumpToLatest)) {
          setState(() {
            _followLatest = true;
            _showJumpToLatest = false;
          });
        }
      }
    });
  }

  Future<void> _send() async {
    final text = _inputCtrl.text.trim();
    if (text.isEmpty || _sessionEnded || _isSending) return;
    await _ensureSession();
    if (!mounted) return;
    final sessionId = _sessionId;
    if (sessionId == null || _sessionEnded) {
      setState(() {
        _canSend = _inputCtrl.text.trim().isNotEmpty;
        _sendError = _copy.sendFailed(StateError('Miku is not connected yet.'));
      });
      return;
    }
    final messageId =
        _pendingMessageText == text && _pendingMessageId != null
            ? _pendingMessageId!
            : newClientMessageId();
    final sendEpoch = ++_sendEpoch;
    setState(() {
      _isSending = true;
      _canSend = false;
      _sendError = '';
      _pendingMessageId = messageId;
      _pendingMessageText = text;
    });
    try {
      await _sendText(
        text,
        clientMessageId: messageId,
        targetSessionId: sessionId,
        sendEpoch: sendEpoch,
      );
      if (!mounted || sendEpoch != _sendEpoch || _sessionId != sessionId) {
        return;
      }
      _inputCtrl.clear();
      setState(() {
        _isSending = false;
        _canSend = false;
        _pendingMessageId = null;
        _pendingMessageText = null;
        _pendingOptimisticRound = null;
      });
    } catch (err) {
      if (!mounted || sendEpoch != _sendEpoch || _sessionId != sessionId) {
        return;
      }
      setState(() {
        _isSending = false;
        _canSend = _inputCtrl.text.trim().isNotEmpty;
        _sendError = _copy.sendFailed(err);
      });
    }
  }

  Future<void> _sendText(
    String text, {
    String? clientMessageId,
    String? targetSessionId,
    int? sendEpoch,
  }) async {
    if (targetSessionId == null) await _ensureSession();
    final sessionId = targetSessionId ?? _sessionId;
    if (sessionId == null) {
      throw StateError('Miku is not connected yet.');
    }
    final operationEpoch = sendEpoch ?? ++_sendEpoch;
    if (_sessionId != sessionId || operationEpoch != _sendEpoch) {
      throw StateError('The active session changed before this send began.');
    }
    if (_sessionEnded) {
      throw StateError('This session has ended.');
    }
    final statusBeforeSend = _status;
    final retryRound =
        clientMessageId != null &&
                _pendingMessageId == clientMessageId &&
                _pendingOptimisticRound != null
            ? _pendingOptimisticRound
            : null;
    final round =
        retryRound ??
        _ConversationRound(index: _rounds.length + 1, userText: text);
    setState(() {
      if (!_rounds.contains(round)) _rounds.add(round);
      round.isStreaming = true;
      if (clientMessageId != null) _pendingOptimisticRound = round;
      _status = 'streaming';
      _canSend = false;
    });
    _scrollToBottom(force: true);
    try {
      await widget.client.sendMessage(
        sessionId,
        text,
        clientMessageId: clientMessageId ?? newClientMessageId(),
      );
    } catch (_) {
      if (!mounted || operationEpoch != _sendEpoch || _sessionId != sessionId) {
        rethrow;
      }
      final hasServerEvidence =
          round.assistantText.isNotEmpty ||
          round.activities.isNotEmpty ||
          round.hasReasoning;
      setState(() {
        round.isStreaming = false;
        if (clientMessageId == null && !hasServerEvidence) {
          _rounds.remove(round);
        }
        _status = statusBeforeSend;
      });
      rethrow;
    }
  }

  Future<void> _resolve(
    ApprovalPrompt a,
    String decision, {
    String? optionId,
  }) async {
    await widget.client.resolveApproval(
      _sessionId!,
      a.approvalId,
      decision,
      optionId: optionId,
    );
    setState(() => _approvals.remove(a));
  }

  ApprovalPrompt? _approvalForProposal(MemoryWriteProposal proposal) {
    for (final approval in _approvals) {
      if (approval.proposalId == proposal.proposalId) return approval;
    }
    return null;
  }

  bool _isRenderedAsMemoryProposal(ApprovalPrompt approval) {
    final proposalId = approval.proposalId;
    if (proposalId == null) return false;
    return approval.isMemoryProposal &&
        _memoryProposals.any((proposal) => proposal.proposalId == proposalId);
  }

  List<ApprovalPrompt> get _driveApprovals =>
      _approvals.where(_isDriveApproval).toList();

  bool _isDriveApproval(ApprovalPrompt approval) {
    if (approval.action.startsWith('drive.')) return true;
    if (approval.backend == 'drive') return true;
    final capability = approval.scope['capability']?.toString() ?? '';
    return capability.startsWith('drive.');
  }

  Future<void> _loadProject() async {
    final id = _sessionId;
    if (id == null) return;
    try {
      final overview = await widget.client.projectOverview(id);
      if (!mounted || _sessionId != id) return;
      setState(() {
        _projectStatus = overview.status;
        _nextActions
          ..clear()
          ..addAll(overview.nextActions);
      });
    } catch (_) {
      // Project context is optional and must never take a healthy chat offline.
    }
  }

  Future<DriveFeed> _fetchDriveFeed() async {
    final id = _sessionId;
    if (id == null) return DriveFeed.empty;
    return widget.client.driveFeed(id, limit: 12);
  }

  Future<void> _loadDriveFeed({bool silent = false}) async {
    final id = _sessionId;
    if (id == null) return;
    if (!silent && mounted) {
      setState(() {
        _driveLoading = true;
        _driveError = '';
      });
    }
    try {
      final feed = await widget.client.driveFeed(id, limit: 12);
      if (!mounted || _sessionId != id) return;
      setState(() {
        _driveFeed = feed;
        _driveLoading = false;
        _driveError = '';
      });
    } catch (err) {
      if (!mounted || _sessionId != id) return;
      setState(() {
        _driveLoading = false;
        _driveError = '$err';
      });
    }
  }

  Future<void> _promoteSession() async {
    await _ensureSession();
    final last =
        _rounds
            .where((round) => round.assistantFinalText.isNotEmpty)
            .lastOrNull;
    final resources = _promotionResources(last?.assistantFinalText ?? '');
    try {
      final p = await widget.client.promoteSession(
        _sessionId!,
        summary: last?.assistantFinalText,
        resources: resources,
      );
      if (!mounted) return;
      setState(
        () => _projectStatus = '${p.projectUri} · ${p.promotedCount} promoted',
      );
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Promote failed: $e')));
    }
  }

  List<String> _promotionResources(String finalText) {
    final resources = <String>[];
    void add(String uri) {
      final normalized = _normalizeResourceUri(uri);
      final supported =
          normalized.startsWith('artifact://') ||
          normalized.startsWith('workspace://session/') ||
          normalized.startsWith('linked://') ||
          (normalized.startsWith('project://') &&
              normalized.contains('/workspace/'));
      if (!supported || resources.contains(normalized)) return;
      resources.add(normalized);
    }

    for (final uri in _extractResources(finalText)) {
      add(uri);
    }
    for (final round in _rounds) {
      for (final activity in round.activities) {
        for (final uri in activity.resourceUris) {
          add(uri);
        }
      }
    }
    return resources;
  }

  Future<void> _openResource(String uri) async {
    await _ensureSession();
    final normalized = _normalizeResourceUri(uri);
    if (normalized.isEmpty) return;
    try {
      final preview = await widget.client.resolveResource(
        _sessionId!,
        normalized,
      );
      if (!mounted) return;
      await showModalBottomSheet<void>(
        context: context,
        showDragHandle: true,
        isScrollControlled: true,
        backgroundColor: _tok.surface,
        builder:
            (_) => _ResourceSheet(preview: preview, tok: _tok, copy: _copy),
      );
    } catch (err) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not open $normalized: $err')),
      );
    }
  }

  Future<void> _applyModePick(String id) async {
    await _ensureSession();
    final sessionId = _sessionId;
    if (sessionId == null) return;
    final previousId = _modeId;
    setState(() => _modeId = id);
    try {
      if (_modeLocked) {
        await widget.client.lockMode(sessionId, id);
      } else {
        await widget.client.overrideMode(sessionId, id);
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _modeId = previousId);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Mode change failed: $err')));
    }
  }

  Future<void> _toggleModeLock() async {
    await _ensureSession();
    final sessionId = _sessionId;
    if (sessionId == null) return;
    final wasLocked = _modeLocked;
    setState(() => _modeLocked = !wasLocked);
    try {
      if (wasLocked) {
        await widget.client.unlockMode(sessionId);
      } else {
        await widget.client.lockMode(sessionId, _modeId);
      }
    } catch (err) {
      if (!mounted) return;
      setState(() => _modeLocked = wasLocked);
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('Mode lock failed: $err')));
    }
  }

  Future<void> _loadHistoricalSession(String sessionId) async {
    if (_disconnecting) return;
    final navigationEpoch = _nextSessionNavigationEpoch();
    _sessionFuture = null;
    if (mounted) {
      setState(() {
        _status = 'connecting';
        _isSending = false;
        _canSend = false;
        _sendError = '';
      });
    }
    try {
      final loaded = await widget.client.loadSession(sessionId);
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
      await _attachSession(
        loaded.session,
        messages: loaded.messages,
        pendingEvents: loaded.pendingEvents,
        navigationEpoch: navigationEpoch,
      );
    } catch (err) {
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
      setState(() => _status = 'offline');
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('History load failed: $err')));
    }
  }

  Future<bool> _startNewSession({String? initialMessage}) async {
    if (_disconnecting) return false;
    final navigationEpoch = _nextSessionNavigationEpoch();
    _sessionFuture = null;
    if (mounted) {
      setState(() {
        _status = 'connecting';
        _isSending = false;
        _canSend = false;
        _sendError = '';
      });
    }
    try {
      final session = await widget.client.createSession();
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return false;
      LoadedSession? loaded;
      if (initialMessage != null) {
        await widget.client.sendMessage(
          session.id,
          initialMessage,
          clientMessageId: newClientMessageId(),
        );
        try {
          loaded = await widget.client.loadSession(session.id);
        } catch (_) {
          loaded = null;
        }
      }
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return false;
      await _attachSession(
        loaded?.session ?? session,
        messages: loaded?.messages ?? const [],
        pendingEvents: loaded?.pendingEvents ?? const [],
        navigationEpoch: navigationEpoch,
      );
      return true;
    } catch (err) {
      if (!mounted || navigationEpoch != _sessionNavigationEpoch) return false;
      setState(() => _status = 'offline');
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text('New session failed: $err')));
      return false;
    }
  }

  // ── Bottom sheets ──────────────────────────────────────────────────────────

  void _showModeSheet() {
    final tok = _tok;
    if (_modes.isEmpty) {
      unawaited(_loadModes());
    }
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      isScrollControlled: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder:
          (sheetContext) => ConstrainedBox(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.of(sheetContext).size.height * 0.9,
            ),
            child: _ModeSheet(
              modes: _modes.isEmpty ? [_mode] : List<_Mode>.from(_modes),
              currentId: _modeId,
              locked: _modeLocked,
              tok: tok,
              copy: _copy,
              onPick: (id) {
                Navigator.pop(sheetContext);
                unawaited(_applyModePick(id));
              },
              onLockToggle: () {
                Navigator.pop(sheetContext);
                unawaited(_toggleModeLock());
              },
            ),
          ),
    );
  }

  void _showApprovalSheet(ApprovalPrompt a) {
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: _tok.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder:
          (_) => _ApprovalSheet(
            approval: a,
            tok: _tok,
            copy: _copy,
            accent: _accent,
            onOption: (option) {
              final isReject =
                  option.kind.startsWith('reject') ||
                  option.kind.startsWith('deny');
              _resolve(
                a,
                isReject ? 'deny' : 'approve',
                optionId: option.optionId,
              );
              Navigator.pop(context);
            },
            onApprove: () {
              _resolve(a, 'approve');
              Navigator.pop(context);
            },
            onDeny: () {
              _resolve(a, 'deny');
              Navigator.pop(context);
            },
          ),
    );
  }

  void _showActivitySheet(_ConversationRound round) {
    final tok = _tok;
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      isScrollControlled: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder:
          (sheetContext) => ConstrainedBox(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.of(sheetContext).size.height * 0.9,
            ),
            child: _AgentActivitySheet(
              tok: tok,
              copy: _copy,
              accent: _accent,
              roundIndex: round.index,
              agents: _agentStatuses(round.activities),
              activities: List<_ActivityItem>.from(round.activities),
              onOpenResource: _openResource,
            ),
          ),
    );
  }

  void _showDriveSheet() {
    final tok = _tok;
    unawaited(_loadDriveFeed(silent: true));
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      isScrollControlled: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder:
          (sheetContext) => SizedBox(
            height: MediaQuery.of(sheetContext).size.height * 0.9,
            child: _DriveFeedSheet(
              tok: tok,
              copy: _copy,
              accent: _accent,
              initialFeed: _driveFeed,
              initialError: _driveError,
              initialLoading: _driveLoading,
              approvals: _driveApprovals,
              loadFeed: () async {
                final feed = await _fetchDriveFeed();
                if (mounted) {
                  setState(() {
                    _driveFeed = feed;
                    _driveError = '';
                    _driveLoading = false;
                  });
                }
                return feed;
              },
              onOpenResource: _openResource,
              onOpenApproval: (approval) {
                Navigator.pop(sheetContext);
                Timer(const Duration(milliseconds: 320), () {
                  if (mounted) _showApprovalSheet(approval);
                });
              },
            ),
          ),
    );
  }

  Future<void> _showServerTargetDialog(ServerTargetClient client) async {
    final copy = _copy;
    String initial;
    try {
      initial = await client.serverBaseUrl();
    } catch (_) {
      initial = '';
    }
    if (!mounted) return;
    final scan = await showDialog<bool>(
      context: context,
      builder:
          (dialogContext) => AlertDialog(
            backgroundColor: _tok.surface,
            title: Text(copy.serverTarget),
            content: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(copy.serverUrl),
                const SizedBox(height: 6),
                SelectableText(initial.isEmpty ? 'Not paired' : initial),
                const SizedBox(height: 18),
                const Text(
                  'Changing servers requires a fresh one-time pairing code. '
                  'Manual URL-only pairing is disabled.',
                ),
              ],
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(dialogContext, false),
                child: Text(copy.cancel),
              ),
              FilledButton(
                onPressed: () => Navigator.pop(dialogContext, true),
                child: const Text('Scan QR'),
              ),
            ],
          ),
    );
    if (scan != true || !mounted) return;
    final rawLink = await Navigator.of(context).push<String>(
      MaterialPageRoute(builder: (_) => const PairingScannerPage()),
    );
    if (rawLink != null && rawLink.trim().isNotEmpty) {
      await _applyPairingLink(rawLink);
    }
  }

  Future<void> _reconnectAfterPair({String? successMessage}) async {
    final navigationEpoch = _nextSessionNavigationEpoch();
    final previousSub = _sub;
    _sub = null;
    if (previousSub != null) unawaited(previousSub.cancel());
    _sessionFuture = null;
    _inputCtrl.clear();
    if (mounted) {
      setState(() {
        _sessionId = null;
        _lastEventId = null;
        _status = 'connecting';
        _canSend = false;
        _isSending = false;
        _disconnecting = false;
        _sendError = '';
        _pendingMessageId = null;
        _pendingMessageText = null;
        _pendingOptimisticRound = null;
        _approvals.clear();
        _memoryProposals.clear();
        _rounds.clear();
        _nextActions.clear();
        _projectStatus = '';
        _driveFeed = null;
        _driveError = '';
        _voiceAsrSelection = VoiceAsrEngineKind.local;
        _voiceAsrCatalog = VoiceAsrEngineCatalog.localOnly();
      });
    }
    final future = _connectSession(navigationEpoch);
    _sessionFuture = future;
    await future;
    await _refreshVoiceAsrEngines();
    if (successMessage != null &&
        mounted &&
        navigationEpoch == _sessionNavigationEpoch) {
      _showSnack(successMessage);
    }
  }

  void _startFreshChat() {
    if (_disconnecting) return;
    unawaited(_startNewSession());
  }

  void _retryConnection() {
    final sessionId = _sessionId;
    if (sessionId != null) {
      unawaited(_loadHistoricalSession(sessionId));
      return;
    }
    _sessionFuture = null;
    unawaited(_ensureSession());
  }

  Future<void> _startPairingScan() async {
    if (_serverTargetClient == null) return;
    final rawLink = await Navigator.of(context).push<String>(
      MaterialPageRoute(builder: (_) => const PairingScannerPage()),
    );
    if (rawLink != null && rawLink.trim().isNotEmpty) {
      await _applyPairingLink(rawLink);
    }
  }

  Future<void> _disconnectFromServer(ServerTargetClient client) async {
    final approved = await showDialog<bool>(
      context: context,
      builder:
          (dialogContext) => AlertDialog(
            title: Text(_copy.pick('Disconnect from Miku?', '與 Miku 中斷連線？')),
            content: Text(
              _copy.pick(
                'This removes the device credential. You will need to scan a new one-time QR before chatting again.',
                '這會移除裝置憑證。再次聊天前，必須掃描新的一次性 QR。',
              ),
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(dialogContext, false),
                child: Text(_copy.cancel),
              ),
              FilledButton(
                style: FilledButton.styleFrom(
                  backgroundColor: _tok.danger,
                  foregroundColor: _textOn(_tok.danger),
                ),
                onPressed: () => Navigator.pop(dialogContext, true),
                child: Text(_copy.pick('Disconnect', '中斷連線')),
              ),
            ],
          ),
    );
    if (approved != true) return;
    if (!await _beginServerAuthorityTransition()) return;
    final navigationEpoch = _nextSessionNavigationEpoch();
    _sessionFuture = null;
    final previousSub = _sub;
    _sub = null;
    if (previousSub != null) unawaited(previousSub.cancel());
    if (mounted) {
      setState(() {
        _status = 'connecting';
        _canSend = false;
        _isSending = false;
        _sendError = '';
      });
    }
    Object? logoutError;
    try {
      await client.logout();
    } catch (error) {
      logoutError = error;
    }
    try {
      await _syncInlineReplyAuthority();
    } catch (_) {
      // The server credential is already gone; native reply authority is cleared again on boot.
    }
    if (!mounted || navigationEpoch != _sessionNavigationEpoch) return;
    _inputCtrl.clear();
    setState(() {
      _needsPairing = true;
      _sessionId = null;
      _lastEventId = null;
      _status = 'offline';
      _canSend = false;
      _isSending = false;
      _disconnecting = false;
      _sendError = '';
      _pendingMessageId = null;
      _pendingMessageText = null;
      _pendingOptimisticRound = null;
      _approvals.clear();
      _memoryProposals.clear();
      _rounds.clear();
      _nextActions.clear();
      _projectStatus = '';
      _driveFeed = null;
      _driveError = '';
    });
    if (logoutError != null) {
      _showSnack(
        _copy.pick(
          'Local credential removed. The server could not confirm logout: $logoutError',
          '本機憑證已移除，但 Server 無法確認登出：$logoutError',
        ),
      );
    }
  }

  // ── Build ──────────────────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    final tok = _tok;
    final accent = _accent;
    final isDark = Theme.of(context).brightness == Brightness.dark;
    final serverTargetClient = _serverTargetClient;
    final themeController = MikuThemeScope.controllerOf(context);

    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: isDark ? SystemUiOverlayStyle.light : SystemUiOverlayStyle.dark,
      child: Scaffold(
        backgroundColor: tok.bg,
        drawer: Builder(
          builder:
              (drawerCtx) => _MikuDrawer(
                tok: tok,
                copy: _copy,
                accent: accent,
                currentSessionId: _sessionId,
                loadSessions: widget.client.listSessions,
                onSelect: (id) {
                  Scaffold.of(drawerCtx).closeDrawer();
                  if (_sessionId != id || _sessionEnded) {
                    unawaited(_loadHistoricalSession(id));
                  }
                },
                onNewSession: () {
                  Scaffold.of(drawerCtx).closeDrawer();
                  _startFreshChat();
                },
                onDrive: () {
                  Scaffold.of(drawerCtx).closeDrawer();
                  Timer(const Duration(milliseconds: 320), () {
                    if (mounted) _showDriveSheet();
                  });
                },
                refreshToken: _sessionHistoryRevision,
                projectStatus: _projectStatus,
                nextActions: _nextActions,
                approvals: _approvals,
                onOpenApproval: (a) {
                  Scaffold.of(drawerCtx).closeDrawer();
                  Timer(const Duration(milliseconds: 320), () {
                    if (mounted) _showApprovalSheet(a);
                  });
                },
                onPromote: () {
                  Scaffold.of(drawerCtx).closeDrawer();
                  unawaited(_promoteSession());
                },
                onRefresh: () => unawaited(_loadProject()),
                themeMode: themeController.mode,
                onThemeModeChanged:
                    (mode) => unawaited(themeController.setMode(mode)),
                onLanguageToggle: () => unawaited(_toggleLanguage()),
                onModeSettings: () {
                  Scaffold.of(drawerCtx).closeDrawer();
                  Timer(const Duration(milliseconds: 320), () {
                    if (mounted) _showModeSheet();
                  });
                },
                voiceModelStatus: _voiceModelStatus,
                voiceAsrSummary: _selectedVoiceAsrSummary,
                onVoiceAsr: () {
                  Scaffold.of(drawerCtx).closeDrawer();
                  Timer(const Duration(milliseconds: 320), () {
                    if (mounted) unawaited(_showVoiceAsrDialog());
                  });
                },
                onVoiceModel: () {
                  Scaffold.of(drawerCtx).closeDrawer();
                  Timer(const Duration(milliseconds: 320), () {
                    if (mounted) unawaited(_showVoiceModelDialog());
                  });
                },
                onServerTarget:
                    serverTargetClient == null
                        ? null
                        : () {
                          Scaffold.of(drawerCtx).closeDrawer();
                          Timer(const Duration(milliseconds: 320), () {
                            if (mounted) {
                              _showServerTargetDialog(serverTargetClient);
                            }
                          });
                        },
                onDisconnect:
                    serverTargetClient == null
                        ? null
                        : () {
                          Scaffold.of(drawerCtx).closeDrawer();
                          unawaited(_disconnectFromServer(serverTargetClient));
                        },
              ),
        ),
        body: DecoratedBox(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [tok.bg, tok.surface, tok.bg],
              stops: const [0, 0.58, 1],
            ),
          ),
          child: SafeArea(
            child:
                _needsPairing
                    ? _PairingWelcome(
                      tok: tok,
                      copy: _copy,
                      brand: const MikuBrandBadge(size: 92),
                      onScan: _startPairingScan,
                    )
                    : Column(
                      children: [
                        _MikuTopBar(tok: tok, copy: _copy, status: _status),
                        _ConnectionBanner(
                          tok: tok,
                          copy: _copy,
                          status: _status,
                          onRetry: _retryConnection,
                          onNewSession: _startFreshChat,
                        ),
                        Expanded(
                          child: _MikuChatSurface(
                            tok: tok,
                            copy: _copy,
                            accent: accent,
                            status: _status,
                            scrollController: _scrollCtrl,
                            dotAnimation: _dotAnim,
                            rounds: _rounds,
                            memoryProposals: _memoryProposals,
                            approvals: _approvals,
                            showJumpToLatest: _showJumpToLatest,
                            approvalForProposal: _approvalForProposal,
                            isRenderedAsMemoryProposal:
                                _isRenderedAsMemoryProposal,
                            onJumpToLatest:
                                () =>
                                    _scrollToBottom(force: true, animate: true),
                            onShowActivity: _showActivitySheet,
                            onOpenResource:
                                (uri) => unawaited(_openResource(uri)),
                            onResolve:
                                (approval, decision) =>
                                    unawaited(_resolve(approval, decision)),
                            onShowApproval: _showApprovalSheet,
                          ),
                        ),
                        _MikuComposer(
                          tok: tok,
                          copy: _copy,
                          accent: accent,
                          controller: _inputCtrl,
                          sessionEnded: _sessionEnded,
                          isSending: _isSending,
                          canSend: _canSend,
                          sendError: _sendError,
                          voiceSupported: widget.voiceCapture.isSupported,
                          voiceEngineReady:
                              _selectedVoiceAsrReady && !_disconnecting,
                          voiceRecording: _voiceRecording,
                          voiceProcessing: _voiceProcessing,
                          onChanged: _handleComposerChanged,
                          onSend: () => unawaited(_send()),
                          onVoiceStart: () => unawaited(_startVoiceCapture()),
                          onVoiceStop: () => unawaited(_stopVoiceCapture()),
                          onVoiceCancel: () => unawaited(_cancelVoiceCapture()),
                        ),
                      ],
                    ),
          ),
        ),
      ),
    );
  }

  void _handleComposerChanged(String value) {
    final text = value.trim();
    final shouldSend = !_sessionEnded && text.isNotEmpty;
    final changedPending =
        _pendingMessageText != null && _pendingMessageText != text;
    if (shouldSend == _canSend && !changedPending && _sendError.isEmpty) {
      return;
    }
    setState(() {
      _canSend = shouldSend;
      if (changedPending) {
        final optimisticRound = _pendingOptimisticRound;
        if (optimisticRound != null &&
            optimisticRound.assistantText.isEmpty &&
            optimisticRound.activities.isEmpty &&
            !optimisticRound.hasReasoning) {
          _rounds.remove(optimisticRound);
        }
        _pendingMessageId = null;
        _pendingMessageText = null;
        _pendingOptimisticRound = null;
      }
      _sendError = '';
    });
  }
}
