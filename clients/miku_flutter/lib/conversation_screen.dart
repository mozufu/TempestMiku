import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/semantics.dart';

import 'asr/local_asr_engine.dart';
import 'asr/local_asr_model.dart';
import 'conversation_notifications.dart';
import 'notification_service.dart';
import 'pairing_scanner.dart';
import 'rich_message.dart';
import 'session_models.dart';
import 'share_import_service.dart';
import 'theme_mode_controller.dart';
import 'voice_capture_service.dart';

part 'conversation_project_browser.dart';
part 'conversation_project_scope.dart';
part 'conversation_drive.dart';
part 'conversation_history.dart';
part 'conversation_session_context.dart';
part 'conversation_settings.dart';
part 'conversation_settings_sections.dart';
part 'conversation_resources.dart';

part 'conversation_reviewed_changes.dart';
part 'conversation_reviewed_change_dialogs.dart';
part 'conversation_import_review.dart';
part 'conversation_voice.dart';
part 'conversation_voice_engines.dart';
part 'conversation_items.dart';
part 'conversation_rows.dart';
part 'conversation_drawer.dart';
part 'conversation_approval_card.dart';
part 'conversation_composer.dart';
part 'conversation_event_fidelity.dart';
part 'conversation_event_labels.dart';

const _showRichResponseShowcase = bool.fromEnvironment(
  'TM_RICH_RESPONSE_SHOWCASE',
);

class ConversationScreen extends StatefulWidget {
  const ConversationScreen({
    required this.client,
    required this.themeModeController,
    this.now,
    this.shareImports,
    this.voiceCapture,
    this.localAsrWorkers,
    this.localAsrModels,
    this.notifications,
    this.voiceInferenceTimeout = const Duration(seconds: 45),
    super.key,
  });

  final MikuSessionClient client;
  final MikuThemeModeController themeModeController;
  final DateTime Function()? now;
  final MikuShareImportService? shareImports;
  final MikuVoiceCaptureService? voiceCapture;
  final LocalAsrWorkerFactory? localAsrWorkers;
  final LocalAsrModelManager? localAsrModels;
  final MikuNotificationService? notifications;
  final Duration voiceInferenceTimeout;

  @override
  State<ConversationScreen> createState() => _ConversationScreenState();
}

enum _PresenceState { loading, here, working, reconnecting, offline, ended }

enum _ServerConnectionState { connecting, connected, reconnecting, offline }

class _ConversationScreenState extends State<ConversationScreen>
    with WidgetsBindingObserver {
  final _scaffoldKey = GlobalKey<ScaffoldState>();
  final _composerController = TextEditingController();
  final _composerFocus = FocusNode();
  final _scrollController = ScrollController();
  bool _showScrollToBottom = false;
  bool _reviewedChangeInFlight = false;
  final List<_ConversationItem> _items = [];
  _MessageItem? _streamingAssistant;
  final Map<String, bool> _activityGroupExpanded = {};
  final List<SharedContent> _pendingImports = [];
  final List<String> _recentImportEventIds = [];
  final Map<String, Set<String>> _trackedTurnIdsBySession = {};
  final Map<String, String> _observedTurnStatuses = {};
  final Map<String, String?> _observedTurnErrors = {};
  final Set<String> _refreshingTurnIds = {};

  StreamSubscription<MikuEvent>? _eventSubscription;
  StreamSubscription<SharedContent>? _shareImportSubscription;
  late final MikuShareImportService _shareImports;
  late final MikuVoiceCaptureService _voiceCapture;
  late final LocalAsrModelManager _localAsrModels;
  late final BackgroundNotificationCoordinator _notificationCoordinator;
  late final Future<VoiceAppBuildFingerprint?> _voiceBuildFingerprint;
  LocalAsrTranscriber? _voiceTranscriber;
  LocalAsrModelStatus? _voiceModelStatus;
  VoiceAsrEngineCatalog _voiceAsrCatalog = VoiceAsrEngineCatalog.localOnly();
  VoiceAsrEngineKind _voiceAsrSelection = VoiceAsrEngineKind.local;
  VoiceAsrEngineKind? _activeVoiceAsrSelection;
  AppLifecycleState _appLifecycle = AppLifecycleState.resumed;
  Timer? _voiceLimitTimer;
  String? _voiceCaptureId;
  String? _voiceError;
  int _voiceOperationEpoch = 0;
  int _serverAuthorityEpoch = 0;
  bool _voiceRecording = false;
  bool _voiceProcessing = false;
  bool _voicePermissionPending = false;
  bool _voiceAsrCatalogLoading = false;
  MikuSession? _session;
  _PresenceState _presence = _PresenceState.loading;
  _ServerConnectionState _serverConnection = _ServerConnectionState.connecting;
  bool _sending = false;
  String? _connectionError;
  bool _modeCatalogLoading = false;
  ModeCatalog? _modeCatalog;
  String? _modeCatalogError;
  String? _changingModeId;
  int _connectionGeneration = 0;
  int _localSequence = 0;
  bool _initialConnectionComplete = false;
  bool _processingImports = false;
  ValueNotifier<SharedContent>? _activeImport;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _notificationCoordinator = BackgroundNotificationCoordinator(
      client: widget.client,
      notifications: widget.notifications ?? createNotificationService(),
      onOpenSession: _openNotificationSession,
      onOpenApproval: _openNotificationApproval,
      onConfirmLegacyAction: _confirmLegacyNotificationAction,
      onQuietNotice: _showNotificationNotice,
      isApprovalInFlight: _isApprovalItemInFlight,
    );
    unawaited(_notificationCoordinator.initialize());
    _shareImports = widget.shareImports ?? createShareImportService();
    _voiceCapture = widget.voiceCapture ?? createVoiceCaptureService();
    _localAsrModels = widget.localAsrModels ?? createLocalAsrModelManager();
    _voiceTranscriber =
        widget.localAsrWorkers == null
            ? null
            : LocalAsrTranscriber(
              workers: widget.localAsrWorkers!,
              timeout: widget.voiceInferenceTimeout,
            );
    if (widget.localAsrWorkers != null) {
      _voiceModelStatus = const LocalAsrModelStatus(
        state: LocalAsrModelState.ready,
        reason: 'injected worker factory',
        encoder: 'injected',
        decoder: 'injected',
        tokens: 'injected',
      );
    } else if (_localAsrModels.isSupported) {
      unawaited(_refreshVoiceModel().catchError((_) => null));
    }
    _voiceBuildFingerprint = _inspectVoiceBuild();
    if (_voiceCapture.isSupported) {
      unawaited(_voiceCapture.recoverOrphans().catchError((_) => 0));
    }
    _scrollController.addListener(_updateScrollToBottomVisibility);
    if (_shareImports.isSupported) {
      _shareImportSubscription = _shareImports.imports.listen(
        _enqueueImport,
        onError: (_) {
          if (!mounted) return;
          ScaffoldMessenger.of(
            context,
          ).showSnackBar(const SnackBar(content: Text('收到的分享內容無法讀取。')));
        },
      );
    }
    unawaited(_connect());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _eventSubscription?.cancel();
    _shareImportSubscription?.cancel();
    _notificationCoordinator.dispose();
    _voiceLimitTimer?.cancel();
    _voiceOperationEpoch += 1;
    unawaited(_voiceCapture.cancel(_voiceCaptureId).catchError((_) => false));
    if (_activeVoiceAsrSelection == VoiceAsrEngineKind.remote) {
      unawaited(widget.client.cancelVoiceAsrTranscription());
    }
    final transcriber = _voiceTranscriber;
    if (transcriber != null) unawaited(transcriber.cancel());
    _voiceTranscriber = null;
    _composerController.dispose();
    _composerFocus.dispose();
    _scrollController.removeListener(_updateScrollToBottomVisibility);
    _scrollController.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _appLifecycle = state;
    _notificationCoordinator.setLifecycleState(state);
    if (state != AppLifecycleState.resumed &&
        (_voiceRecording || (_voiceProcessing && !_voicePermissionPending))) {
      unawaited(_cancelVoiceCapture());
    }
  }

  void _voiceSetState(VoidCallback update) => setState(update);

  void _cancelEventStream() {
    final subscription = _eventSubscription;
    _eventSubscription = null;
    if (subscription != null) unawaited(subscription.cancel());
  }

  String _nextKey(String prefix) => '$prefix-${_localSequence++}';

  Future<void> _connect({
    bool createNew = false,
    String? sessionId,
    String? newSessionProjectId,
    MikuMemoryPolicy? newSessionMemoryPolicy,
  }) async {
    assert(!createNew || sessionId == null);
    final generation = ++_connectionGeneration;
    if (mounted) {
      setState(() {
        _presence = _PresenceState.loading;
        _serverConnection = _ServerConnectionState.connecting;
        _connectionError = null;
        _sending = false;
      });
    }
    try {
      late final LoadedSession loaded;
      if (createNew) {
        final session = await widget.client.createSession(
          projectId: newSessionProjectId,
          memoryPolicy: newSessionMemoryPolicy,
        );
        _cancelEventStream();
        loaded = await widget.client.loadSession(session.id);
      } else if (sessionId != null) {
        _cancelEventStream();
        loaded = await widget.client.loadSession(sessionId);
      } else {
        _cancelEventStream();
        final session = await widget.client.createOrReuseSession();
        loaded = await widget.client.loadSession(session.id);
      }
      if (!mounted || generation != _connectionGeneration) return;
      final restored = <_ConversationItem>[
        for (final message in loaded.messages)
          _MessageItem(
            key: 'history-${message.seq}',
            role: message.role,
            text: message.content,
          ),
        if (_showRichResponseShowcase)
          _MessageItem(
            key: 'rich-response-showcase',
            role: 'assistant',
            text: mikuRichResponseShowcase,
          ),
      ];
      setState(() {
        _session = loaded.session;
        _items
          ..clear()
          ..addAll(restored);
        _presence =
            loaded.session.status == 'ended'
                ? _PresenceState.ended
                : _PresenceState.here;
        _serverConnection = _ServerConnectionState.connected;
      });
      for (final event in loaded.pendingEvents) {
        _handleEvent(event, remember: false);
      }
      if (_presence != _PresenceState.ended) {
        _listenForEvents(loaded.session.id, loaded.session.lastEventId);
      }
      unawaited(_refreshVoiceAsrEngines());
      unawaited(_recoverTrackedTurns(loaded.session.id));
      _scheduleScroll(force: true);
    } catch (error) {
      if (!mounted || generation != _connectionGeneration) return;
      setState(() {
        _presence = _PresenceState.offline;
        _serverConnection = _ServerConnectionState.offline;
        _connectionError = _friendlyError(error);
      });
    } finally {
      if (mounted && generation == _connectionGeneration) {
        _initialConnectionComplete = true;
        _notificationCoordinator.setInitialConnectionComplete();
        unawaited(_drainImports());
      }
    }
  }

  void _startNewConversation() {
    _composerController.clear();
    unawaited(_connect(createNew: true));
  }

  Future<bool> _startProjectConversation(ProjectCatalogEntry project) async {
    final previousSessionId = _session?.id;
    _composerController.clear();
    await _connect(
      createNew: true,
      newSessionProjectId: project.id,
      newSessionMemoryPolicy: project.defaultMemoryPolicy,
    );
    final session = _session;
    return session != null &&
        session.id != previousSessionId &&
        session.projectId == project.id &&
        session.memoryPolicy == project.defaultMemoryPolicy &&
        _presence != _PresenceState.offline;
  }

  Future<bool> _prepareDeviceAuthorityMutation(
    bool preserveNotificationIntent,
  ) async {
    final voiceCleaned = await _prepareForAuthorityMutation();
    if (!voiceCleaned) return false;
    return _notificationCoordinator.prepareAuthorityChange(
      preserveIntent: preserveNotificationIntent,
    );
  }

  Future<void> _openNotificationSession(String sessionId) async {
    if (!mounted || sessionId.trim().isEmpty) return;
    if (_session?.id != sessionId) {
      await _connect(sessionId: sessionId);
    }
  }

  Future<void> _openNotificationApproval(
    String sessionId,
    ApprovalDetails approval,
  ) async {
    if (!mounted ||
        sessionId.trim().isEmpty ||
        approval.sessionId != sessionId ||
        !approval.isPending) {
      return;
    }
    if (_session?.id != sessionId) {
      await _connect(sessionId: sessionId);
    }
    if (!mounted || _session?.id != sessionId) return;
    setState(() {
      if (!_items.whereType<_ApprovalItem>().any(
        (item) => item.prompt.approvalId == approval.approvalId,
      )) {
        _items.add(
          _ApprovalItem(
            key: 'approval-${approval.approvalId}',
            prompt: approval.prompt,
          ),
        );
      }
      _presence = _PresenceState.here;
    });
    _scheduleScroll(force: true);
  }

  Future<bool> _confirmLegacyNotificationAction(
    ApprovalNotificationAction action,
    ApprovalDetails approval,
  ) async {
    if (!mounted || approval.sessionId != action.sessionId) return false;
    final approving = action.decision == 'approve';
    return await showDialog<bool>(
          context: context,
          builder:
              (context) => AlertDialog(
                title: Text(approving ? '確認允許？' : '確認拒絕？'),
                content: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 480),
                  child: Text('${approval.action}\n\n伺服器已重新確認這個核准仍在等待中。'),
                ),
                actions: [
                  TextButton(
                    onPressed: () => Navigator.of(context).pop(false),
                    child: const Text('取消'),
                  ),
                  FilledButton(
                    key: const Key('confirm-legacy-notification-action'),
                    style:
                        approving
                            ? null
                            : FilledButton.styleFrom(
                              backgroundColor:
                                  Theme.of(context).colorScheme.error,
                              foregroundColor:
                                  Theme.of(context).colorScheme.onError,
                            ),
                    onPressed: () => Navigator.of(context).pop(true),
                    child: Text(approving ? '確認允許' : '確認拒絕'),
                  ),
                ],
              ),
        ) ??
        false;
  }

  void _showNotificationNotice(String message) {
    if (!mounted || message.trim().isEmpty) return;
    setState(() {
      _items.add(
        _NoticeItem(
          key: _nextKey('notification-notice'),
          text: message,
          isError: true,
        ),
      );
    });
    _scheduleScroll();
  }

  Future<void> _openSettings() async {
    final result = await showModalBottomSheet<_SettingsResult>(
      context: context,
      useSafeArea: true,
      isScrollControlled: true,
      showDragHandle: true,
      builder:
          (context) => _SettingsSheet(
            client: widget.client,
            themeModeController: widget.themeModeController,
            voiceSupported: _voiceCapture.isSupported,
            initialVoiceModelStatus: _voiceModelStatus,
            initialVoiceCatalog: _voiceAsrCatalog,
            initialVoiceSelection: _voiceAsrSelection,
            onPrepareDeviceAuthorityChange: _prepareDeviceAuthorityMutation,
            onAuthorityChangeCommitted:
                _notificationCoordinator.commitAuthorityChange,
            onAuthorityChangeAborted:
                _notificationCoordinator.abortAuthorityChange,
            onRefreshVoiceModel: _refreshVoiceModel,
            onInstallVoiceModel: _installVoiceModel,
            onDeleteVoiceModel: _deleteVoiceModel,
            onRefreshVoiceCatalog:
                () => _refreshVoiceAsrEngines(allowFallback: false),
            onSelectVoiceEngine: _selectVoiceAsrEngine,
            notificationSettingsPanel: BackgroundNotificationsSettingsPanel(
              coordinator: _notificationCoordinator,
            ),
          ),
    );
    if (!mounted || result == null) return;
    if (result == _SettingsResult.paired) {
      _clearAuthorityBoundUi();
      await _connect();
      return;
    }
    _clearAuthorityBoundUi(connectionError: '已登出這台裝置。重新配對後即可繼續。');
  }

  void _clearAuthorityBoundUi({String? connectionError}) {
    _cancelEventStream();
    _resetVoiceAuthorityState();
    _composerController.clear();
    _pendingImports.clear();
    _trackedTurnIdsBySession.clear();
    _observedTurnStatuses.clear();
    _observedTurnErrors.clear();
    _refreshingTurnIds.clear();
    setState(() {
      _session = null;
      _items.clear();
      _streamingAssistant = null;
      _presence = _PresenceState.offline;
      _serverConnection = _ServerConnectionState.offline;
      _connectionError = connectionError;
      _modeCatalog = null;
      _modeCatalogError = null;
      _modeCatalogLoading = false;
    });
  }

  Future<void> _openResources() async {
    final session = _session;
    if (session == null) {
      ScaffoldMessenger.of(context)
        ..hideCurrentSnackBar()
        ..showSnackBar(const SnackBar(content: Text('先建立對話，才能讀取授權資源。')));
      return;
    }
    await showModalBottomSheet<void>(
      context: context,
      useSafeArea: true,
      isScrollControlled: true,
      showDragHandle: true,
      builder:
          (context) => _ResourceInspectorSheet(
            client: widget.client,
            sessionId: session.id,
          ),
    );
  }

  void _openSessionContext() {
    _scaffoldKey.currentState?.openEndDrawer();
    if (_modeCatalog == null) unawaited(_loadModeCatalog());
  }

  Future<void> _loadModeCatalog() async {
    if (_modeCatalogLoading) return;
    setState(() {
      _modeCatalogLoading = true;
      _modeCatalogError = null;
    });
    try {
      final catalog = await widget.client.modeCatalog();
      if (!mounted) return;
      setState(() => _modeCatalog = catalog);
    } catch (_) {
      if (!mounted) return;
      setState(() => _modeCatalogError = 'Mode 清單暫時讀不到，請再試一次。');
    } finally {
      if (mounted) setState(() => _modeCatalogLoading = false);
    }
  }

  Future<void> _switchMode(ModeProfile profile) async {
    final session = _session;
    if (session == null ||
        session.status == 'ended' ||
        _changingModeId != null ||
        profile.id == session.mode) {
      return;
    }
    setState(() {
      _changingModeId = profile.id;
      _modeCatalogError = null;
    });
    try {
      await widget.client.overrideMode(session.id, profile.id);
      if (!mounted || _session?.id != session.id) return;
      setState(() {
        _session = _sessionWithMode(session, profile: profile, locked: false);
      });
    } catch (_) {
      if (!mounted || _session?.id != session.id) return;
      setState(() => _modeCatalogError = 'Mode 沒有切換，請再試一次。');
    } finally {
      if (mounted && _session?.id == session.id) {
        setState(() => _changingModeId = null);
      }
    }
  }

  Future<void> _setModeLocked(bool locked) async {
    final session = _session;
    if (session == null ||
        session.status == 'ended' ||
        _changingModeId != null ||
        session.locked == locked) {
      return;
    }
    setState(() {
      _changingModeId = session.mode;
      _modeCatalogError = null;
    });
    try {
      if (locked) {
        await widget.client.lockMode(session.id, session.mode);
      } else {
        await widget.client.unlockMode(session.id);
      }
      if (!mounted || _session?.id != session.id) return;
      setState(() => _session = _copySessionWithLock(session, locked));
    } catch (_) {
      if (!mounted || _session?.id != session.id) return;
      setState(() => _modeCatalogError = 'Mode 鎖定狀態沒有變更，請再試一次。');
    } finally {
      if (mounted && _session?.id == session.id) {
        setState(() => _changingModeId = null);
      }
    }
  }

  Future<void> _endCurrentSession() async {
    final session = _session;
    if (session == null || session.status == 'ended') return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder:
          (context) => AlertDialog(
            title: const Text('結束這段對話？'),
            content: const Text('對話與事件會保留為唯讀記錄；之後請開新對話才能繼續傳送訊息。'),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(context).pop(false),
                child: const Text('取消'),
              ),
              FilledButton(
                key: const Key('confirm-end-session'),
                onPressed: () => Navigator.of(context).pop(true),
                child: const Text('結束對話'),
              ),
            ],
          ),
    );
    if (confirmed != true || !mounted) return;
    try {
      await widget.client.endSession(session.id);
      if (!mounted || _session?.id != session.id) return;
      setState(() {
        _session = _copySessionWithStatus(session, 'ended');
        _presence = _PresenceState.ended;
      });
      _cancelEventStream();
    } catch (_) {
      if (!mounted || _session?.id != session.id) return;
      setState(() => _connectionError = '沒有結束這段對話，請再試一次。');
    }
  }

  void _openDrivePage() {
    final session = _session;
    if (session == null) return;
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _DrivePage(client: widget.client, session: session),
      ),
    );
  }

  void _openProjectPage() {
    final session = _session;
    if (session == null) return;
    Navigator.of(context).push(
      MaterialPageRoute(
        builder:
            (_) => _ProjectPage(
              client: widget.client,
              session: session,
              sessionEnded: _presence == _PresenceState.ended,
              onMemoryContextChanged: _applyMemoryContext,
              onNewConversation: _startProjectConversation,
            ),
      ),
    );
  }

  void _openHistoryPage() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder:
            (_) => _HistoryPage(
              client: widget.client,
              currentSessionId: _session?.id,
              onSelectSession: (sessionId) {
                Navigator.of(context).pop();
                _openHistorySession(sessionId);
              },
            ),
      ),
    );
  }

  /// Applies committed project and memory-policy changes from the project page.
  void _applyMemoryContext(String? projectId, MikuMemoryPolicy memoryPolicy) {
    final session = _session;
    if (session == null ||
        (session.projectId == projectId &&
            session.memoryPolicy == memoryPolicy)) {
      return;
    }
    setState(
      () =>
          _session = session.copyWith(
            projectId: projectId,
            memoryPolicy: memoryPolicy,
          ),
    );
  }

  void _openHistorySession(String sessionId) {
    if (sessionId == _session?.id) return;
    _composerController.clear();
    unawaited(_connect(sessionId: sessionId));
  }

  void _listenForEvents(String sessionId, String? lastEventId) {
    _eventSubscription = widget.client
        .events(sessionId, lastEventId: lastEventId)
        .listen(
          _handleEvent,
          onError: (Object error, StackTrace stackTrace) {
            if (!mounted) return;
            setState(() {
              _presence = _PresenceState.offline;
              _serverConnection = _ServerConnectionState.offline;
              _connectionError = _friendlyError(error);
            });
          },
          onDone: () {
            if (!mounted || _presence == _PresenceState.ended) return;
            setState(() {
              _presence = _PresenceState.offline;
              _serverConnection = _ServerConnectionState.offline;
              _connectionError = '連線中斷了。你的對話仍然保留著。';
            });
          },
        );
  }

  void _handleEvent(MikuEvent event, {bool remember = true}) {
    if (!mounted) return;
    final notificationApproval =
        event.type == 'approval' ? _approvalFromEvent(event) : null;
    final resolvedNotificationApprovalId =
        event.type == 'approval_resolved'
            ? _string(event.data['approvalId'])
            : '';
    final eventTurnId =
        event.turnId ??
        (_string(event.data['turnId']).isEmpty
            ? null
            : _string(event.data['turnId']));
    setState(() {
      if (eventTurnId != null) {
        _applyTurnEvent(eventTurnId, event);
      }
      switch (event.type) {
        case 'text':
          _appendTextDelta(_string(event.data['delta']));
          _presence = _PresenceState.working;
        case 'final':
          _finishAssistantMessage(_string(event.data['text']));
          _finishActivities();
          _presence = _PresenceState.here;
        case 'tool_call':
          _addActivity(event, '正在處理', detail: _string(event.data['name']));
          _presence = _PresenceState.working;
        case 'cell_start':
          _addActivity(event, '正在執行', detail: '安全工作環境');
          _presence = _PresenceState.working;
        case 'effect_start':
          _addActivity(event, '正在執行', detail: '受控能力');
          _presence = _PresenceState.working;
        case 'effect_suspended':
          _pauseActivity(event);
          _presence = _PresenceState.here;
        case 'effect_resumed':
          _resumeActivity(event);
          _presence = _PresenceState.working;
        case 'actor_spawned':
          _addActivity(event, '正在分工處理', detail: _string(event.data['task']));
          _presence = _PresenceState.working;
        case 'reasoning':
          _addActivity(event, '正在想一想');
          _presence = _PresenceState.working;
        case 'progress':
          _addActivity(
            event,
            _string(event.data['label']).isEmpty
                ? '正在處理'
                : _string(event.data['label']),
          );
          _presence = _PresenceState.working;
        case 'cell_result':
        case 'effect_end':
        case 'effect_result':
          final phase = _runtimeTerminalPhase(event);
          _completeActivity(
            event,
            label: _runtimeTerminalLabel(event.type, phase),
            phase: phase,
          );
        case 'actor_completed':
          _completeActivity(
            event,
            label: '分工處理完成',
            links: _activityResourceLinks(event),
          );
        case 'mcp_invocation':
          final status = _eventStatus(event);
          if (status == 'requested') {
            _addActivity(event, '正在查詢外部資源');
            _presence = _PresenceState.working;
          } else {
            final phase = _mcpInvocationTerminalPhase(status);
            _completeActivity(
              event,
              label: _mcpInvocationTerminalLabel(phase),
              phase: phase,
            );
          }
        case 'approval':
          final prompt = _approvalFromEvent(event);
          if (prompt != null &&
              !_items.whereType<_ApprovalItem>().any(
                (item) => item.prompt.approvalId == prompt.approvalId,
              )) {
            _items.add(
              _ApprovalItem(
                key: 'approval-${prompt.approvalId}',
                prompt: prompt,
              ),
            );
          }
          _presence = _PresenceState.here;
        case 'approval_resolved':
          final approvalId = _string(event.data['approvalId']);
          final status = _string(event.data['status']);
          for (final item in _items.whereType<_ApprovalItem>()) {
            if (item.prompt.approvalId == approvalId) {
              item
                ..resolving = false
                ..resolvedStatus = status;
            }
          }
        case 'runtime_reset':
          _items.add(
            _NoticeItem(key: _nextKey('runtime-reset'), text: '執行環境已重新連線。'),
          );
        case 'error':
          _streamingAssistant = null;
          _items.add(
            _NoticeItem(
              key: event.id ?? _nextKey('error'),
              text:
                  _string(event.data['message']).isEmpty
                      ? '這一步沒有完成，可以再試一次。'
                      : _string(event.data['message']),
              isError: true,
            ),
          );
          _presence = _PresenceState.here;
        case 'session_end':
          _finishActivities();
          final session = _session;
          if (session != null) {
            _session = _copySessionWithStatus(session, 'ended');
          }
          _presence = _PresenceState.ended;
        case 'connection':
          _updateConnectionState(_string(event.data['status']));
        case 'mode':
          final session = _session;
          if (session != null) {
            _session = _sessionFromModeEvent(session, event.data);
          }
          _changingModeId = null;
          break;
        case 'display':
        case 'binding_committed':
        case 'scope_start':
        case 'scope_progress':
        case 'scope_result':
        case 'actor_status':
        case 'actor_message':
        case 'actor_failed':
        case 'actor_supervision':
        case 'actor_cancelled':
        case 'actor_resources_linked':
        case 'tool_call_update':
        case 'diff':
        case 'artifact':
        case 'memory_recall':
        case 'dream_queued':
        case 'dream_started':
        case 'dream_progress':
        case 'dream_completed':
        case 'dream_failed':
        case 'cron_run_started':
        case 'cron_run_completed':
        case 'drive_put':
        case 'drive_transduced':
        case 'drive_path_proposed':
        case 'drive_write_proposed':
        case 'drive_filed':
        case 'drive_moved':
        case 'drive_tagged':
        case 'project_linked':
        case 'project_unlinked':
        case 'drive_organizer_started':
        case 'drive_organizer_completed':
        case 'drive_organizer_failed':
        case 'egress_started':
        case 'egress_completed':
        case 'egress_failed':
        case 'egress_denied':
        case 'secret_handle_issued':
          _recordFidelityEvent(event);
        case 'write_proposal':
          _upsertProposal(event);
        default:
          break;
      }
    });
    final session = _session;
    if (notificationApproval != null && session != null) {
      unawaited(
        _notificationCoordinator.showApprovalWhileBackgrounded(
          sessionId: session.id,
          approval: notificationApproval,
          expiresAt:
              _string(event.data['expiresAt']).isEmpty
                  ? null
                  : _string(event.data['expiresAt']),
        ),
      );
    }
    if (resolvedNotificationApprovalId.isNotEmpty) {
      unawaited(
        _notificationCoordinator.cancelApproval(resolvedNotificationApprovalId),
      );
    }
    if (remember &&
        session != null &&
        event.id != null &&
        shouldRememberEventId(event.type, event.data)) {
      widget.client.rememberLastEventId(session.id, event.id!);
    }
    if (event.type == 'session_end') {
      unawaited(_eventSubscription?.cancel());
    }
    if (eventTurnId != null &&
        (event.type == 'final' || event.type == 'error')) {
      unawaited(_refreshTurnUntilTerminal(eventTurnId));
    }
    if (event.type == 'connection' &&
        _string(event.data['status']) == 'connected' &&
        _session != null) {
      unawaited(_recoverTrackedTurns(_session!.id));
    }
    _scheduleScroll();
  }

  void _appendTextDelta(String delta) {
    if (delta.isEmpty) return;
    final active = _streamingAssistant;
    if (active != null && active.streaming) {
      active.text += delta;
      return;
    }
    final item = _MessageItem(
      key: _nextKey('assistant'),
      role: 'assistant',
      text: delta,
      streaming: true,
    );
    _streamingAssistant = item;
    _items.add(item);
  }

  void _finishAssistantMessage(String text) {
    final active = _streamingAssistant;
    _streamingAssistant = null;
    if (active != null && active.streaming) {
      if (text.isNotEmpty) active.text = text;
      active.streaming = false;
      _announceAssistantReply(active.text);
      return;
    }
    if (text.isEmpty) return;
    final last = _items.isEmpty ? null : _items.last;
    if (last is _MessageItem && last.role == 'assistant' && last.text == text) {
      return;
    }
    _items.add(
      _MessageItem(key: _nextKey('assistant'), role: 'assistant', text: text),
    );
    _announceAssistantReply(text);
  }

  void _announceAssistantReply(String text) {
    if (text.isEmpty || !mounted) return;
    final bounded =
        text.length > 4000 ? text.substring(text.length - 4000) : text;
    unawaited(
      SemanticsService.sendAnnouncement(
        View.of(context),
        bounded,
        TextDirection.ltr,
      ),
    );
  }

  void _updateConnectionState(String status) {
    switch (status) {
      case 'connected':
        _serverConnection = _ServerConnectionState.connected;
        _connectionError = null;
        if (_presence == _PresenceState.loading ||
            _presence == _PresenceState.reconnecting ||
            _presence == _PresenceState.offline) {
          _presence = _PresenceState.here;
        }
      case 'reconnecting':
        _serverConnection = _ServerConnectionState.reconnecting;
        _presence = _PresenceState.reconnecting;
        _connectionError = '連線不穩，正在重新連線。';
      case 'offline':
        _serverConnection = _ServerConnectionState.offline;
        _presence = _PresenceState.offline;
        _connectionError = '現在連不上 Miku。你的對話仍然保留著。';
      default:
        break;
    }
  }

  void _finishActivities() {
    for (final item in _items.whereType<_ActivityItem>()) {
      item.running = false;
    }
  }

  Future<void> _send() => _sendContent(_composerController.text.trim());

  Future<void> _sendContent(
    String content, {
    bool preserveComposerDraft = false,
  }) async {
    final session = _session;
    if (session == null ||
        content.isEmpty ||
        _sending ||
        _presence == _PresenceState.loading ||
        _presence == _PresenceState.offline ||
        _presence == _PresenceState.ended) {
      return;
    }
    final clientMessageId = newClientMessageId();
    final item = _MessageItem(
      key: _nextKey('user'),
      role: 'user',
      text: content,
    );
    final turnItem = _TurnItem(
      key: _nextKey('turn'),
      clientMessageId: clientMessageId,
      status: 'submitting',
    );
    setState(() {
      _streamingAssistant = null;
      _items.addAll([item, turnItem]);
      if (!preserveComposerDraft) _composerController.clear();
      _sending = true;
      _connectionError = null;
    });
    _scheduleScroll(force: true);
    try {
      final receipt = await widget.client.sendMessage(
        session.id,
        content,
        clientMessageId: clientMessageId,
      );
      if (!mounted) return;
      setState(() {
        turnItem.turnId = receipt.turnId;
        turnItem.status =
            _observedTurnStatuses[receipt.turnId] ?? receipt.status;
        turnItem.error = _observedTurnErrors[receipt.turnId];
        if (!turnItem.isTerminal) {
          _trackedTurnIdsBySession
              .putIfAbsent(session.id, () => <String>{})
              .add(receipt.turnId);
        }
      });
      if (turnItem.status == 'finalizing' || receipt.isTerminal) {
        unawaited(_refreshTurnUntilTerminal(receipt.turnId));
      }
    } catch (error) {
      if (!mounted || _session?.id != session.id) return;
      setState(() {
        _items.remove(item);
        _items.remove(turnItem);
        if (!preserveComposerDraft) {
          final currentDraft = _composerController.text;
          final restored =
              currentDraft.trim().isEmpty
                  ? content
                  : '$content\n\n$currentDraft';
          _composerController.text = restored;
          _composerController.selection = TextSelection.collapsed(
            offset: restored.length,
          );
        }
        _connectionError =
            preserveComposerDraft
                ? '匯入內容沒有送出去。原本的輸入草稿仍然保留。'
                : '沒有送出去。內容已經放回輸入框。';
      });
    } finally {
      if (mounted) setState(() => _sending = false);
    }
  }

  void _applyTurnEvent(String turnId, MikuEvent event) {
    final nextStatus = switch (event.type) {
      'final' => 'finalizing',
      'error' => 'failed',
      'approval' => 'waiting',
      'effect_suspended' => 'waiting',
      'write_proposal'
          when _string(event.data['status']).toLowerCase() == 'pending' =>
        'waiting',
      'text' ||
      'tool_call' ||
      'tool_call_update' ||
      'diff' ||
      'artifact' ||
      'cell_start' ||
      'cell_result' ||
      'effect_start' ||
      'effect_resumed' ||
      'effect_end' ||
      'effect_result' ||
      'display' ||
      'binding_committed' ||
      'scope_start' ||
      'scope_progress' ||
      'scope_result' ||
      'actor_spawned' ||
      'actor_status' ||
      'actor_message' ||
      'actor_completed' ||
      'actor_failed' ||
      'actor_supervision' ||
      'actor_cancelled' ||
      'actor_resources_linked' ||
      'reasoning' ||
      'progress' ||
      'mcp_invocation' ||
      'memory_recall' ||
      'drive_put' ||
      'drive_transduced' ||
      'drive_path_proposed' ||
      'drive_write_proposed' ||
      'drive_filed' ||
      'drive_moved' ||
      'drive_tagged' ||
      'project_linked' ||
      'project_unlinked' ||
      'drive_organizer_started' ||
      'drive_organizer_completed' ||
      'drive_organizer_failed' ||
      'egress_started' ||
      'egress_completed' ||
      'egress_failed' ||
      'egress_denied' ||
      'secret_handle_issued' ||
      'write_proposal' => 'running',
      _ => null,
    };
    if (nextStatus == null) return;
    final error =
        event.type == 'error' && _string(event.data['message']).isNotEmpty
            ? _string(event.data['message'])
            : null;
    _observedTurnStatuses[turnId] = nextStatus;
    _observedTurnErrors[turnId] = error;
    for (final item in _items.whereType<_TurnItem>()) {
      if (item.turnId != turnId) continue;
      item
        ..status = nextStatus
        ..error = error;
    }
  }

  Future<void> _recoverTrackedTurns(String sessionId) async {
    final turnIds = List<String>.from(
      _trackedTurnIdsBySession[sessionId] ?? const <String>{},
    );
    for (final turnId in turnIds) {
      if (!mounted || _session?.id != sessionId) return;
      await _refreshTurnUntilTerminal(turnId, maxAttempts: 1);
    }
  }

  Future<void> _refreshTurnUntilTerminal(
    String turnId, {
    int maxAttempts = 4,
  }) async {
    final session = _session;
    if (session == null || !_refreshingTurnIds.add(turnId)) return;
    try {
      for (var attempt = 0; attempt < maxAttempts; attempt += 1) {
        try {
          final turn = await widget.client.getTurn(session.id, turnId);
          if (!mounted || _session?.id != session.id) return;
          setState(() => _applyTurnRecord(session.id, turn));
          if (turn.isTerminal) return;
        } catch (_) {
          if (!mounted || _session?.id != session.id) return;
        }
        if (attempt + 1 < maxAttempts) {
          await Future<void>.delayed(
            Duration(milliseconds: 200 * (attempt + 1)),
          );
        }
      }
    } finally {
      _refreshingTurnIds.remove(turnId);
    }
  }

  void _applyTurnRecord(String sessionId, SessionTurn turn) {
    _observedTurnStatuses[turn.id] = turn.status;
    _observedTurnErrors[turn.id] = turn.error;
    _TurnItem? item;
    for (final candidate in _items.whereType<_TurnItem>()) {
      if (candidate.turnId == turn.id) {
        item = candidate;
        break;
      }
    }
    if (item != null) {
      item
        ..status = turn.status
        ..error = turn.error;
    } else if (!turn.isTerminal || turn.status != 'completed') {
      _items.add(
        _TurnItem(
          key: 'recovered-turn-${turn.id}',
          clientMessageId: turn.clientMessageId,
          turnId: turn.id,
          status: turn.status,
          error: turn.error,
        ),
      );
    }
    final tracked = _trackedTurnIdsBySession.putIfAbsent(
      sessionId,
      () => <String>{},
    );
    if (turn.isTerminal) {
      tracked.remove(turn.id);
    } else {
      tracked.add(turn.id);
    }
  }

  Future<void> _resolveApproval(
    _ApprovalItem item,
    ApprovalOption option,
  ) async {
    final session = _session;
    if (session == null || item.resolving || item.resolvedStatus != null) {
      return;
    }
    final approve = _isApprovalApproveKind(option.kind);
    setState(() {
      item
        ..resolving = true
        ..error = null;
    });
    try {
      await widget.client.resolveApproval(
        session.id,
        item.prompt.approvalId,
        approve ? 'approve' : 'deny',
        optionId: option.optionId,
      );
      await _notificationCoordinator.cancelApproval(item.prompt.approvalId);
    } catch (error) {
      if (!mounted) return;
      setState(() {
        item
          ..resolving = false
          ..error = '沒有完成這個決定，請再試一次。';
      });
    }
  }

  bool _isApprovalItemInFlight(String approvalId) {
    for (final item in _items.whereType<_ApprovalItem>()) {
      if (item.prompt.approvalId == approvalId) {
        return item.resolving || item.resolvedStatus != null;
      }
    }
    return false;
  }

  ApprovalPrompt? _approvalFromEvent(MikuEvent event) {
    final approvalId = _string(event.data['approvalId']);
    if (approvalId.isEmpty) return null;
    final rawScope = event.data['scope'];
    final scope =
        rawScope is Map
            ? rawScope.map((key, value) => MapEntry(key.toString(), value))
            : <String, Object?>{};
    final rawOptions = event.data['options'];
    final options = <ApprovalOption>[];
    if (rawOptions is List) {
      for (final raw in rawOptions.whereType<Map>()) {
        final option = raw.map((key, value) => MapEntry(key.toString(), value));
        final id = _string(option['optionId']);
        if (id.isEmpty) continue;
        options.add(
          ApprovalOption(
            optionId: id,
            name:
                _string(option['name']).isEmpty
                    ? _fallbackOptionName(_string(option['kind']))
                    : _string(option['name']),
            kind: _string(option['kind']),
          ),
        );
      }
    }
    return ApprovalPrompt(
      approvalId: approvalId,
      backend: _string(event.data['backend']),
      action:
          _string(event.data['action']).isEmpty
              ? '需要你的確認'
              : _string(event.data['action']),
      scope: scope,
      options:
          options.isEmpty
              ? const [
                ApprovalOption(
                  optionId: 'allow',
                  name: '允許一次',
                  kind: 'allow_once',
                ),
                ApprovalOption(
                  optionId: 'reject',
                  name: '拒絕',
                  kind: 'reject_once',
                ),
              ]
              : options,
      timeoutMs: event.data['timeoutMs'] as int?,
    );
  }

  void _updateScrollToBottomVisibility() {
    if (!_scrollController.hasClients) return;
    final position = _scrollController.position;
    final nearBottom = position.maxScrollExtent - position.pixels < 160;
    final show = !nearBottom && position.maxScrollExtent > 0;
    if (show != _showScrollToBottom) {
      setState(() => _showScrollToBottom = show);
    }
  }

  void _scheduleScroll({bool force = false}) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_scrollController.hasClients) return;
      final position = _scrollController.position;
      final nearBottom = position.maxScrollExtent - position.pixels < 160;
      if (!force && !nearBottom) return;
      if (MediaQuery.maybeOf(context)?.disableAnimations ?? false) {
        position.jumpTo(position.maxScrollExtent);
        return;
      }
      _scrollController.animateTo(
        position.maxScrollExtent,
        duration: const Duration(milliseconds: 180),
        curve: Curves.easeOutCubic,
      );
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Scaffold(
      key: _scaffoldKey,
      drawer: _ConversationDrawer(
        onOpenSettings: _openSettings,
        onOpenResources: _openResources,
        onOpenReviewedChanges: _openReviewedChanges,
        onNewConversation: _startNewConversation,
        currentSessionId: _session?.id,
        currentSessionEnded: _presence == _PresenceState.ended,
        onOpenDrive: _openDrivePage,
        onOpenProject: _openProjectPage,
        onOpenHistory: _openHistoryPage,
      ),
      drawerEnableOpenDragGesture: true,
      drawerEdgeDragWidth: 32,
      endDrawer: _SessionContextDrawer(
        session: _session,
        catalog: _modeCatalog,
        loading: _modeCatalogLoading,
        error: _modeCatalogError,
        changingModeId: _changingModeId,
        onRetry: _loadModeCatalog,
        onSelectMode: _switchMode,
        onSetLocked: _setModeLocked,
        onEndSession: _endCurrentSession,
      ),
      endDrawerEnableOpenDragGesture: true,
      body: SafeArea(
        child: LayoutBuilder(
          builder: (context, constraints) {
            final horizontalPadding = constraints.maxWidth < 600 ? 16.0 : 28.0;
            return Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 820),
                child: Padding(
                  padding: EdgeInsets.symmetric(horizontal: horizontalPadding),
                  child: Column(
                    children: [
                      _PresenceBar(
                        connection: _serverConnection,
                        session: _session,
                        onOpenDrawer:
                            () => _scaffoldKey.currentState?.openDrawer(),
                        onOpenContext: _openSessionContext,
                      ),
                      Divider(height: 1, color: palette.outline),
                      Expanded(
                        child: Stack(
                          children: [
                            _buildConversation(palette),
                            if (_showScrollToBottom)
                              Positioned(
                                right: 12,
                                bottom: 12,
                                child: FloatingActionButton.small(
                                  key: const Key('scroll-to-bottom'),
                                  tooltip: '捲到最新訊息',
                                  onPressed: () => _scheduleScroll(force: true),
                                  child: const Icon(
                                    Icons.arrow_downward_rounded,
                                  ),
                                ),
                              ),
                          ],
                        ),
                      ),
                      if (_connectionError != null)
                        _ConnectionNotice(
                          text: _connectionError!,
                          canRetry: _presence == _PresenceState.offline,
                          onRetry: _connect,
                          onPair:
                              _session == null &&
                                      widget.client is ServerTargetClient
                                  ? _openSettings
                                  : null,
                        ),
                      _Composer(
                        controller: _composerController,
                        focusNode: _composerFocus,
                        enabled: _canCompose,
                        disabledHint: _disabledComposerHint,
                        sending: _sending,
                        onSend: _send,
                        voiceVisible: _voiceCapture.isSupported,
                        voiceReady: _selectedVoiceAsrReady,
                        voiceRecording: _voiceRecording,
                        voiceProcessing: _voiceProcessing,
                        voiceSummary: _selectedVoiceAsrSummary,
                        voiceError: _voiceError,
                        onVoiceAction:
                            _voiceRecording
                                ? _stopVoiceCapture
                                : _startVoiceCapture,
                        onVoiceCancel: _cancelVoiceCapture,
                      ),
                    ],
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );
  }

  Widget _buildConversation(_Palette palette) {
    if (_presence == _PresenceState.loading && _items.isEmpty) {
      return const Center(child: _QuietLoading());
    }
    if (_items.isEmpty) {
      return Semantics(
        liveRegion: true,
        label: 'Miku is here',
        child: Center(
          child: Padding(
            padding: const EdgeInsets.only(bottom: 48),
            child: Text(
              '${_greeting()}。我在這裡。',
              key: const Key('empty-presence-copy'),
              textAlign: TextAlign.center,
              style: Theme.of(context).textTheme.titleMedium?.copyWith(
                color: palette.muted,
                fontWeight: FontWeight.w400,
              ),
            ),
          ),
        ),
      );
    }
    final nodes = _renderNodes();
    return ListView.builder(
      key: const Key('conversation-list'),
      controller: _scrollController,
      padding: const EdgeInsets.symmetric(vertical: 24),
      keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
      itemCount: nodes.length,
      itemBuilder: (context, index) {
        final node = nodes[index];
        final next = index + 1 < nodes.length ? nodes[index + 1] : null;
        final followedByTurn =
            node is _ItemNode &&
            node.item is _MessageItem &&
            next is _ItemNode &&
            next.item is _TurnItem;
        return Padding(
          padding: EdgeInsets.only(
            bottom:
                followedByTurn
                    ? 6
                    : node is _ItemNode && node.item is _TurnItem
                    ? 14
                    : 18,
          ),
          child: switch (node) {
            _ActivityGroupNode group => _ActivityGroupRow(
              group: group,
              expanded: _isActivityGroupExpanded(group),
              onToggle: () => _toggleActivityGroup(group),
              onOpenResource: _openEventResource,
            ),
            _ItemNode itemNode => switch (itemNode.item) {
              _MessageItem message => _MessageRow(message: message),
              _TurnItem turn => _TurnStatusRow(turn: turn),
              _ActivityItem activity => _ActivityRow(
                activity: activity,
                onOpenResource: _openEventResource,
              ),
              _ProposalItem proposal => _ProposalRow(proposal: proposal),
              _ApprovalItem approval => _ApprovalCard(
                item: approval,
                onSelect: (option) => _resolveApproval(approval, option),
              ),
              _NoticeItem notice => _InlineNotice(notice: notice),
            },
          },
        );
      },
    );
  }

  List<_RenderNode> _renderNodes() {
    final nodes = <_RenderNode>[];
    final run = <_ActivityItem>[];
    void flush() {
      if (run.isEmpty) return;
      if (run.length == 1) {
        nodes.add(_ItemNode(run.first));
      } else {
        nodes.add(_ActivityGroupNode(List.of(run)));
      }
      run.clear();
    }

    for (final item in _items) {
      if (item is _ActivityItem) {
        run.add(item);
      } else {
        flush();
        nodes.add(_ItemNode(item));
      }
    }
    flush();
    return nodes;
  }

  bool _isActivityGroupExpanded(_ActivityGroupNode group) =>
      _activityGroupExpanded[group.key] ?? group.hasActive;

  void _toggleActivityGroup(_ActivityGroupNode group) {
    setState(() {
      _activityGroupExpanded[group.key] = !_isActivityGroupExpanded(group);
    });
  }

  bool get _canCompose =>
      _session != null &&
      _presence != _PresenceState.loading &&
      _presence != _PresenceState.reconnecting &&
      _presence != _PresenceState.offline &&
      _presence != _PresenceState.ended;

  String get _disabledComposerHint => switch (_presence) {
    _PresenceState.loading => '正在找 Miku…',
    _PresenceState.reconnecting => '重新連線後再說…',
    _PresenceState.offline => '重新連線後再說…',
    _PresenceState.ended => '這段對話已結束',
    _PresenceState.here || _PresenceState.working => '告訴 Miku…',
  };

  String _greeting() {
    final hour = (widget.now?.call() ?? DateTime.now()).hour;
    if (hour < 5) return '還沒睡呀';
    if (hour < 11) return '早安';
    if (hour < 18) return '午安';
    return '晚上好';
  }
}

class _Palette {
  const _Palette({
    required this.miku,
    required this.muted,
    required this.outline,
    required this.userBubble,
    required this.warm,
    required this.approvalSurface,
    required this.approvalOutline,
  });

  final Color miku;
  final Color muted;
  final Color outline;
  final Color userBubble;
  final Color warm;
  final Color approvalSurface;
  final Color approvalOutline;

  static _Palette of(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    if (dark) {
      return const _Palette(
        miku: Color(0xff5fd0c5),
        muted: Color(0xff9aa8ae),
        outline: Color(0xff28353b),
        userBubble: Color(0xff1a292f),
        warm: Color(0xffffc786),
        approvalSurface: Color(0xff211c18),
        approvalOutline: Color(0xff5d4934),
      );
    }
    return const _Palette(
      miku: Color(0xff167f78),
      muted: Color(0xff657378),
      outline: Color(0xffd9dfdd),
      userBubble: Color(0xffe4efeb),
      warm: Color(0xff9a5c18),
      approvalSurface: Color(0xfffff7ed),
      approvalOutline: Color(0xffe4c49d),
    );
  }
}

String _string(Object? value) => value?.toString() ?? '';

String _friendlyError(Object error) {
  final message = error.toString().replaceFirst(RegExp(r'^\w+Exception: '), '');
  if (message.trim().isEmpty) return '現在連不上 Miku，請稍後再試。';
  return '現在連不上 Miku。$message';
}

bool _isApprovalApproveKind(String kind) => switch (kind) {
  'allow_once' || 'allow_always' || 'allow_session' || 'approve' => true,
  _ => false,
};

String _fallbackOptionName(String kind) {
  if (_isApprovalApproveKind(kind)) return '允許一次';
  return '拒絕';
}

String _approvalResolutionLabel(String status) => switch (status) {
  'approved' => '已允許',
  'denied' => '已拒絕',
  'timed_out' => '已逾時，未執行',
  'cancelled' => '已取消，未執行',
  _ => '已結束：$status',
};

String? _scopeLabel(Map<String, Object?> scope) {
  final capability = _string(scope['capability']);
  final actor = _string(scope['actorId']);
  if (capability.isNotEmpty && actor.isNotEmpty) return '$actor · $capability';
  if (capability.isNotEmpty) return capability;
  final proposal = scope['proposal'];
  if (proposal is Map) {
    final text = _string(proposal['text']);
    if (text.isNotEmpty) return text;
  }
  return null;
}
