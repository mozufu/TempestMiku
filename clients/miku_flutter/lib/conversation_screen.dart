import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

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
part 'conversation_drive.dart';
part 'conversation_history.dart';
part 'conversation_session_context.dart';
part 'conversation_settings.dart';
part 'conversation_resources.dart';
part 'conversation_reviewed_changes.dart';
part 'conversation_import_review.dart';
part 'conversation_voice.dart';
part 'conversation_event_fidelity.dart';

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

sealed class _ConversationItem {
  const _ConversationItem(this.key);

  final String key;
}

class _MessageItem extends _ConversationItem {
  _MessageItem({
    required String key,
    required this.role,
    required this.text,
    this.streaming = false,
  }) : super(key);

  final String role;
  String text;
  bool streaming;
}

class _ActivityItem extends _ConversationItem {
  _ActivityItem({
    required String key,
    required this.label,
    this.detail,
    this.correlationKey,
    this.phase = _ActivityPhase.running,
    List<_ActivityResourceLink> links = const [],
  }) : links = List.of(links),
       super(key);

  String label;
  String? detail;
  final String? correlationKey;
  _ActivityPhase phase;
  final List<_ActivityResourceLink> links;

  bool get running => phase == _ActivityPhase.running;

  set running(bool value) {
    if (value) {
      phase = _ActivityPhase.running;
    } else if (phase == _ActivityPhase.running) {
      phase = _ActivityPhase.completed;
    }
  }
}

class _TurnItem extends _ConversationItem {
  _TurnItem({
    required String key,
    required this.clientMessageId,
    required this.status,
    this.turnId,
    this.error,
  }) : super(key);

  final String clientMessageId;
  String status;
  String? turnId;
  String? error;

  bool get isTerminal =>
      const {'completed', 'failed', 'cancelled', 'timed_out'}.contains(status);
}

class _ApprovalItem extends _ConversationItem {
  _ApprovalItem({required String key, required this.prompt}) : super(key);

  final ApprovalPrompt prompt;
  bool resolving = false;
  String? resolvedStatus;
  String? error;
}

class _NoticeItem extends _ConversationItem {
  const _NoticeItem({
    required String key,
    required this.text,
    this.isError = false,
  }) : super(key);

  final String text;
  final bool isError;
}

sealed class _RenderNode {
  const _RenderNode();
}

class _ItemNode extends _RenderNode {
  const _ItemNode(this.item);

  final _ConversationItem item;
}

class _ActivityGroupNode extends _RenderNode {
  _ActivityGroupNode(this.activities);

  final List<_ActivityItem> activities;

  String get key => activities.first.correlationKey ?? activities.first.key;

  bool get hasActive => activities.any(
    (a) =>
        a.phase == _ActivityPhase.running || a.phase == _ActivityPhase.paused,
  );
}

class _ConversationScreenState extends State<ConversationScreen>
    with WidgetsBindingObserver {
  final _scaffoldKey = GlobalKey<ScaffoldState>();
  final _composerController = TextEditingController();
  final _composerFocus = FocusNode();
  final _scrollController = ScrollController();
  bool _showScrollToBottom = false;
  bool _reviewedChangeInFlight = false;
  final List<_ConversationItem> _items = [];
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
    _composerController.addListener(_composerChanged);
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
    _composerController
      ..removeListener(_composerChanged)
      ..dispose();
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

  void _composerChanged() {
    if (mounted) setState(() {});
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
    String newSessionScope = 'global',
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
          scope: newSessionScope,
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
    await _connect(createNew: true, newSessionScope: project.memoryScope);
    final session = _session;
    return session != null &&
        session.id != previousSessionId &&
        session.defaultScope == project.memoryScope &&
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
              onScopeChanged: _applyScope,
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

  /// Applies a committed memory-scope change reported by the project page so the composer and
  /// session-context surfaces reflect the session's new project (or Global).
  void _applyScope(String scope) {
    final session = _session;
    if (session == null || session.defaultScope == scope) return;
    setState(() => _session = _sessionWithScope(session, scope));
  }

  MikuSession _sessionWithScope(MikuSession session, String scope) {
    return MikuSession(
      id: session.id,
      status: session.status,
      mode: session.mode,
      label: session.label,
      defaultScope: scope,
      activeSkills: session.activeSkills,
      lastEventId: session.lastEventId,
      locked: session.locked,
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
    final last = _items.isEmpty ? null : _items.last;
    if (last is _MessageItem && last.role == 'assistant' && last.streaming) {
      last.text += delta;
      return;
    }
    _items.add(
      _MessageItem(
        key: _nextKey('assistant'),
        role: 'assistant',
        text: delta,
        streaming: true,
      ),
    );
  }

  void _finishAssistantMessage(String text) {
    for (final item in _items.reversed) {
      if (item is _MessageItem && item.role == 'assistant' && item.streaming) {
        if (text.isNotEmpty) item.text = text;
        item.streaming = false;
        return;
      }
    }
    if (text.isEmpty) return;
    final last = _items.isEmpty ? null : _items.last;
    if (last is _MessageItem && last.role == 'assistant' && last.text == text) {
      return;
    }
    _items.add(
      _MessageItem(key: _nextKey('assistant'), role: 'assistant', text: text),
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
    final approve =
        option.kind.contains('allow') || option.kind.contains('approve');
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

class _PresenceBar extends StatelessWidget {
  const _PresenceBar({
    required this.connection,
    required this.session,
    required this.onOpenDrawer,
    required this.onOpenContext,
  });

  final _ServerConnectionState connection;
  final MikuSession? session;
  final VoidCallback onOpenDrawer;
  final VoidCallback onOpenContext;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final status = switch (connection) {
      _ServerConnectionState.connecting => '正在連上伺服器',
      _ServerConnectionState.connected => '伺服器已連線',
      _ServerConnectionState.reconnecting => '正在重新連線',
      _ServerConnectionState.offline => '伺服器未連線',
    };
    return Semantics(
      container: true,
      liveRegion: true,
      label: status,
      child: SizedBox(
        height: 68,
        child: Row(
          children: [
            IconButton(
              key: const Key('open-left-drawer'),
              tooltip: '開啟對話選單',
              onPressed: onOpenDrawer,
              icon: const Icon(Icons.menu_rounded),
            ),
            const SizedBox(width: 4),
            _PresenceMark(
              active: connection == _ServerConnectionState.connected,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'Miku',
                    style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 1),
                  Text(
                    status,
                    key: const Key('presence-status'),
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                ],
              ),
            ),
            IconButton(
              key: const Key('open-session-context'),
              tooltip: '開啟對話狀態',
              onPressed: session == null ? null : onOpenContext,
              icon: const Icon(Icons.tune_rounded),
            ),
          ],
        ),
      ),
    );
  }
}

class _ConversationDrawer extends StatelessWidget {
  const _ConversationDrawer({
    required this.onOpenSettings,
    required this.onOpenResources,
    required this.onOpenReviewedChanges,
    required this.onNewConversation,
    required this.currentSessionId,
    required this.currentSessionEnded,
    required this.onOpenDrive,
    required this.onOpenProject,
    required this.onOpenHistory,
  });

  final VoidCallback onOpenSettings;
  final VoidCallback onOpenResources;
  final VoidCallback onOpenReviewedChanges;
  final VoidCallback onNewConversation;
  final String? currentSessionId;
  final bool currentSessionEnded;
  final VoidCallback onOpenDrive;
  final VoidCallback onOpenProject;
  final VoidCallback onOpenHistory;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final hasSession = currentSessionId != null;
    return Drawer(
      key: const Key('left-conversation-drawer'),
      backgroundColor: Theme.of(context).colorScheme.surface,
      child: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 10, 8, 8),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      'Miku',
                      key: const Key('left-drawer-title'),
                      style: Theme.of(context).textTheme.titleLarge?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                  IconButton(
                    key: const Key('close-left-drawer'),
                    tooltip: '關閉對話選單',
                    onPressed: () => Navigator.of(context).pop(),
                    icon: const Icon(Icons.close_rounded),
                  ),
                ],
              ),
            ),
            Divider(height: 1, color: palette.outline),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(8, 14, 8, 12),
                children: [
                  _DrawerPageDestination(
                    pageKey: const Key('drawer-drive'),
                    icon: Icons.folder_open_rounded,
                    label: 'Drive',
                    subtitle: 'Miku 的空間',
                    enabled: hasSession,
                    onTap: onOpenDrive,
                  ),
                  _DrawerPageDestination(
                    pageKey: const Key('drawer-project'),
                    icon: Icons.workspaces_outline,
                    label: 'Project',
                    subtitle: '主題實體與工作範圍',
                    enabled: hasSession,
                    onTap: onOpenProject,
                  ),
                  _DrawerPageDestination(
                    pageKey: const Key('drawer-history'),
                    icon: Icons.history_rounded,
                    label: 'History',
                    subtitle: '過往對話與指派',
                    enabled: true,
                    onTap: onOpenHistory,
                  ),
                  ListTile(
                    key: const Key('drawer-resources'),
                    minTileHeight: 52,
                    leading: const Icon(Icons.inventory_2_outlined),
                    title: const Text('Resources'),
                    subtitle: const Text('進階唯讀檢視'),
                    trailing: const Icon(Icons.chevron_right_rounded),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(12),
                    ),
                    onTap: () {
                      Navigator.of(context).pop();
                      onOpenResources();
                    },
                  ),
                  ListTile(
                    key: const Key('drawer-reviewed-changes'),
                    minTileHeight: 52,
                    leading: const Icon(Icons.rule_folder_outlined),
                    title: const Text('經審核的變更'),
                    subtitle: const Text('記憶、guidance 與 rollback'),
                    trailing: const Icon(Icons.chevron_right_rounded),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(12),
                    ),
                    enabled: hasSession && !currentSessionEnded,
                    onTap:
                        !hasSession || currentSessionEnded
                            ? null
                            : () {
                              Navigator.of(context).pop();
                              onOpenReviewedChanges();
                            },
                  ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(14, 12, 14, 18),
              child: Row(
                children: [
                  Expanded(
                    child: OutlinedButton.icon(
                      key: const Key('drawer-settings'),
                      onPressed: () {
                        Navigator.of(context).pop();
                        onOpenSettings();
                      },
                      icon: const Icon(Icons.settings_outlined, size: 19),
                      label: const Text('設定'),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: FilledButton.icon(
                      key: const Key('drawer-new-conversation'),
                      onPressed: () {
                        onNewConversation();
                        Navigator.of(context).pop();
                      },
                      icon: const Icon(Icons.add_comment_outlined, size: 19),
                      label: const Text('新對話'),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _DrawerPageDestination extends StatelessWidget {
  const _DrawerPageDestination({
    required this.pageKey,
    required this.icon,
    required this.label,
    required this.subtitle,
    required this.enabled,
    required this.onTap,
  });

  final Key pageKey;
  final IconData icon;
  final String label;
  final String subtitle;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Semantics(
        button: true,
        label: label,
        child: ListTile(
          key: pageKey,
          minTileHeight: 52,
          leading: Icon(icon),
          title: Text(label),
          subtitle: Text(subtitle),
          trailing: const Icon(Icons.chevron_right_rounded, size: 20),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
          enabled: enabled,
          onTap:
              enabled
                  ? () {
                    Navigator.of(context).pop();
                    onTap();
                  }
                  : null,
        ),
      ),
    );
  }
}

class _DrawerLoadingState extends StatelessWidget {
  const _DrawerLoadingState({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      child: Row(
        children: [
          const SizedBox.square(
            dimension: 15,
            child: CircularProgressIndicator(strokeWidth: 1.8),
          ),
          const SizedBox(width: 10),
          Text(label, style: Theme.of(context).textTheme.bodySmall),
        ],
      ),
    );
  }
}

class _DrawerErrorState extends StatelessWidget {
  const _DrawerErrorState({required this.error, required this.onRetry});

  final String error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.error;
    return Semantics(
      liveRegion: true,
      child: Row(
        children: [
          Expanded(
            child: Text(
              error,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: color),
            ),
          ),
          IconButton(
            tooltip: '重試',
            onPressed: onRetry,
            icon: const Icon(Icons.refresh_rounded, size: 19),
          ),
        ],
      ),
    );
  }
}

class _DrawerEmptyState extends StatelessWidget {
  const _DrawerEmptyState({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 7, 12, 11),
      child: Text(
        text,
        style: Theme.of(
          context,
        ).textTheme.bodySmall?.copyWith(color: _Palette.of(context).muted),
      ),
    );
  }
}

class _PresenceMark extends StatelessWidget {
  const _PresenceMark({required this.active});

  final bool active;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      key: const Key('miku-presence-mark'),
      width: 34,
      height: 34,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: palette.miku.withValues(alpha: active ? 0.16 : 0.07),
        border: Border.all(
          color: palette.miku.withValues(alpha: active ? 0.7 : 0.25),
        ),
      ),
      alignment: Alignment.center,
      child: Container(
        width: 9,
        height: 9,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: active ? palette.miku : palette.muted,
        ),
      ),
    );
  }
}

class _MessageRow extends StatelessWidget {
  const _MessageRow({required this.message});

  final _MessageItem message;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final user = message.role == 'user';
    final body =
        user
            ? SelectableText(
              message.text,
              key: Key('message-${message.key}'),
              style: Theme.of(context).textTheme.bodyLarge,
            )
            : MikuRichMessage(
              key: Key('message-${message.key}'),
              data: message.text,
            );
    return Semantics(
      liveRegion: message.streaming,
      label: user ? '你說：${message.text}' : null,
      child: Align(
        alignment: user ? Alignment.centerRight : Alignment.centerLeft,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: user ? 560 : 690),
          child:
              user
                  ? DecoratedBox(
                    decoration: BoxDecoration(
                      color: palette.userBubble,
                      borderRadius: const BorderRadius.only(
                        topLeft: Radius.circular(20),
                        topRight: Radius.circular(7),
                        bottomLeft: Radius.circular(20),
                        bottomRight: Radius.circular(20),
                      ),
                      border: Border.all(color: palette.outline),
                    ),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 16,
                        vertical: 11,
                      ),
                      child: body,
                    ),
                  )
                  : Padding(
                    padding: const EdgeInsets.only(left: 3, right: 16),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.end,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Flexible(child: body),
                        if (message.streaming) ...[
                          const SizedBox(width: 7),
                          _StreamingDot(color: palette.miku),
                        ],
                      ],
                    ),
                  ),
        ),
      ),
    );
  }
}

class _StreamingDot extends StatefulWidget {
  const _StreamingDot({required this.color});

  final Color color;

  @override
  State<_StreamingDot> createState() => _StreamingDotState();
}

class _StreamingDotState extends State<_StreamingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 900),
      lowerBound: 0.35,
      upperBound: 1,
    )..repeat(reverse: true);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context)) {
      _controller.stop();
    } else if (!_controller.isAnimating) {
      _controller.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return FadeTransition(
      opacity: _controller,
      child: Container(
        width: 6,
        height: 6,
        decoration: BoxDecoration(color: widget.color, shape: BoxShape.circle),
      ),
    );
  }
}

class _TurnStatusRow extends StatelessWidget {
  const _TurnStatusRow({required this.turn});

  final _TurnItem turn;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final failed = const {
      'failed',
      'cancelled',
      'timed_out',
    }.contains(turn.status);
    final complete = turn.status == 'completed';
    final label = switch (turn.status) {
      'submitting' => '正在送到伺服器',
      'queued' => '已排入安全佇列',
      'running' => 'Miku 正在處理',
      'waiting' => '等待你的確認',
      'finalizing' => '回覆已收到，正在確認保存',
      'completed' => '已完成並保存',
      'failed' => '處理失敗',
      'cancelled' => '已取消',
      'timed_out' => '處理逾時',
      _ => '伺服器狀態：${turn.status}',
    };
    final color = failed ? Theme.of(context).colorScheme.error : palette.muted;
    return Semantics(
      liveRegion: !turn.isTerminal,
      label: '$label${turn.error == null ? '' : '，${turn.error}'}',
      child: Align(
        alignment: Alignment.centerRight,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 560),
          child: Padding(
            padding: const EdgeInsets.only(right: 4),
            child: Row(
              key: Key('turn-status-${turn.key}'),
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Padding(
                  padding: const EdgeInsets.only(top: 2),
                  child:
                      complete
                          ? Icon(
                            Icons.cloud_done_outlined,
                            size: 15,
                            color: palette.miku,
                          )
                          : failed
                          ? Icon(
                            Icons.error_outline_rounded,
                            size: 15,
                            color: color,
                          )
                          : SizedBox.square(
                            dimension: 13,
                            child: CircularProgressIndicator(
                              strokeWidth: 1.5,
                              color: palette.miku,
                            ),
                          ),
                ),
                const SizedBox(width: 7),
                Flexible(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        label,
                        style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color: color,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                      if (turn.error != null && turn.error!.trim().isNotEmpty)
                        Text(
                          turn.error!,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(
                            context,
                          ).textTheme.bodySmall?.copyWith(color: color),
                        ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _ActivityRow extends StatelessWidget {
  const _ActivityRow({required this.activity, required this.onOpenResource});

  final _ActivityItem activity;
  final ValueChanged<String> onOpenResource;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      key: Key('activity-${activity.correlationKey ?? activity.key}'),
      liveRegion: activity.running || activity.phase == _ActivityPhase.paused,
      label:
          '${activity.label}${activity.detail == null ? '' : '，${activity.detail}'}',
      child: Padding(
        padding: const EdgeInsets.only(left: 3),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: SizedBox(
                width: 12,
                height: 12,
                child: _ActivityStatusMark(activity: activity),
              ),
            ),
            const SizedBox(width: 9),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    activity.label,
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: palette.muted,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  if (activity.detail != null)
                    Text(
                      activity.detail!,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: palette.muted.withValues(alpha: 0.78),
                      ),
                    ),
                  if (activity.links.isNotEmpty) ...[
                    const SizedBox(height: 3),
                    Wrap(
                      spacing: 4,
                      runSpacing: 2,
                      children: [
                        for (final link in activity.links)
                          TextButton.icon(
                            key: Key(
                              'activity-resource-${link.kind}-${link.uri}',
                            ),
                            style: TextButton.styleFrom(
                              minimumSize: const Size(0, 44),
                              padding: const EdgeInsets.symmetric(
                                horizontal: 8,
                              ),
                              tapTargetSize: MaterialTapTargetSize.padded,
                              visualDensity: VisualDensity.standard,
                            ),
                            onPressed: () => onOpenResource(link.uri),
                            icon: Icon(link.icon, size: 17),
                            label: Text(link.label),
                          ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ActivityGroupRow extends StatelessWidget {
  const _ActivityGroupRow({
    required this.group,
    required this.expanded,
    required this.onToggle,
    required this.onOpenResource,
  });

  final _ActivityGroupNode group;
  final bool expanded;
  final VoidCallback onToggle;
  final ValueChanged<String> onOpenResource;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final activities = group.activities;
    final active = group.hasActive;
    final anyFailed = activities.any(
      (a) =>
          a.phase == _ActivityPhase.failed ||
          a.phase == _ActivityPhase.cancelled,
    );
    final current = activities.lastWhere(
      (a) =>
          a.phase == _ActivityPhase.running ||
          a.phase == _ActivityPhase.paused,
      orElse: () => activities.last,
    );
    final headerLabel =
        active
            ? current.label
            : anyFailed
            ? '想一想過程（部分未完成）'
            : '想一想過程';
    final Widget mark;
    if (active) {
      mark = CircularProgressIndicator(strokeWidth: 1.6, color: palette.miku);
    } else if (anyFailed) {
      mark = Icon(
        Icons.error_outline_rounded,
        size: 13,
        color: Theme.of(context).colorScheme.error,
      );
    } else {
      mark = Icon(Icons.check_rounded, size: 13, color: palette.miku);
    }
    return Semantics(
      key: Key('activity-group-${group.key}'),
      button: true,
      liveRegion: active,
      label:
          '$headerLabel，${activities.length} 個步驟，${expanded ? '已展開' : '已收合'}',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            key: Key('activity-group-toggle-${group.key}'),
            onTap: onToggle,
            borderRadius: BorderRadius.circular(8),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 10, horizontal: 3),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.center,
                children: [
                  SizedBox(width: 12, height: 12, child: mark),
                  const SizedBox(width: 9),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          headerLabel,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.bodySmall?.copyWith(
                            color: palette.muted,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                        Text(
                          '${activities.length} 個步驟',
                          style: Theme.of(context).textTheme.bodySmall?.copyWith(
                            color: palette.muted.withValues(alpha: 0.78),
                          ),
                        ),
                      ],
                    ),
                  ),
                  Icon(
                    expanded
                        ? Icons.expand_less_rounded
                        : Icons.expand_more_rounded,
                    size: 18,
                    color: palette.muted,
                  ),
                ],
              ),
            ),
          ),
          if (expanded)
            Padding(
              padding: const EdgeInsets.only(left: 12, top: 6),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  for (var i = 0; i < activities.length; i++) ...[
                    if (i > 0) const SizedBox(height: 12),
                    _ActivityRow(
                      activity: activities[i],
                      onOpenResource: onOpenResource,
                    ),
                  ],
                ],
              ),
            ),
        ],
      ),
    );
  }
}

class _ApprovalCard extends StatelessWidget {
  const _ApprovalCard({required this.item, required this.onSelect});

  final _ApprovalItem item;
  final ValueChanged<ApprovalOption> onSelect;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final resolved = item.resolvedStatus;
    final memoryProposal = MemoryWriteProposal.fromApproval(item.prompt);
    final evolutionProposal = EvolutionReviewProposal.fromEvent({
      ...item.prompt.scope,
      'status': 'pending',
    });
    final rollbackProposal = _rollbackReviewDetails(item.prompt.scope);
    final genericScope =
        memoryProposal == null &&
                evolutionProposal == null &&
                rollbackProposal == null
            ? _scopeLabel(item.prompt.scope)
            : null;
    return Semantics(
      liveRegion: true,
      container: true,
      label: '需要確認：${item.prompt.action}',
      child: Container(
        key: Key('approval-${item.prompt.approvalId}'),
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: palette.approvalSurface,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(color: palette.approvalOutline),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.shield_outlined, size: 18, color: palette.warm),
                const SizedBox(width: 8),
                Text('需要你的確認', style: Theme.of(context).textTheme.labelLarge),
              ],
            ),
            const SizedBox(height: 10),
            SelectableText(
              item.prompt.action,
              style: Theme.of(context).textTheme.bodyMedium,
            ),
            if (genericScope case final scope?) ...[
              const SizedBox(height: 5),
              Text(
                scope,
                style: Theme.of(
                  context,
                ).textTheme.bodySmall?.copyWith(color: palette.muted),
              ),
            ],
            if (memoryProposal case final proposal?) ...[
              const SizedBox(height: 12),
              _MemoryProposalDetails(proposal: proposal),
            ] else if (evolutionProposal case final proposal?) ...[
              const SizedBox(height: 12),
              _EvolutionProposalDetails(proposal: proposal),
            ] else if (rollbackProposal case final proposal?) ...[
              const SizedBox(height: 12),
              _RollbackProposalDetails(details: proposal),
            ],
            if (item.error != null) ...[
              const SizedBox(height: 8),
              Text(
                item.error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            ],
            const SizedBox(height: 14),
            if (resolved != null)
              Text(
                _approvalResolutionLabel(resolved),
                key: const Key('approval-resolution'),
                style: Theme.of(context).textTheme.labelLarge?.copyWith(
                  color: resolved == 'approved' ? palette.miku : palette.muted,
                ),
              )
            else
              Wrap(
                spacing: 10,
                runSpacing: 8,
                children: [
                  for (final option in item.prompt.options)
                    _ApprovalButton(
                      option: option,
                      enabled: !item.resolving,
                      onPressed: () => onSelect(option),
                    ),
                  if (item.resolving)
                    const Padding(
                      padding: EdgeInsets.all(10),
                      child: SizedBox.square(
                        dimension: 17,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      ),
                    ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _MemoryProposalDetails extends StatelessWidget {
  const _MemoryProposalDetails({required this.proposal});

  final MemoryWriteProposal proposal;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      key: const Key('memory-proposal-details'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              Chip(
                avatar: const Icon(Icons.psychology_outlined, size: 16),
                label: Text(proposal.kindLabel),
                visualDensity: VisualDensity.compact,
              ),
              Chip(
                label: Text(proposal.scopeLabel),
                visualDensity: VisualDensity.compact,
              ),
            ],
          ),
          const SizedBox(height: 8),
          SelectableText(proposal.displayText),
          const SizedBox(height: 8),
          Text(
            '來源：${proposal.provenanceText}',
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
        ],
      ),
    );
  }
}

class _EvolutionProposalDetails extends StatelessWidget {
  const _EvolutionProposalDetails({required this.proposal});

  final EvolutionReviewProposal proposal;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final targetLabel = proposal.targetKind == 'persona' ? 'Persona' : 'Mode';
    return Container(
      key: const Key('evolution-proposal-details'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.auto_fix_high_outlined, size: 17),
              const SizedBox(width: 7),
              Expanded(
                child: Text(
                  '$targetLabel · ${proposal.targetId}',
                  style: Theme.of(context).textTheme.labelLarge,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          SelectableText(proposal.preview),
          const SizedBox(height: 8),
          Text(
            proposal.applyEnabled
                ? '核准後會建立不可變版本並啟用。'
                : '核准後只保留為 review，不會自動啟用。',
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
          if (proposal.isAutoCandidate && proposal.evidenceCount != null)
            Text(
              '跨對話候選 · ${proposal.evidenceCount} 筆證據',
              style: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(color: palette.muted),
            ),
        ],
      ),
    );
  }
}

class _ApprovalButton extends StatelessWidget {
  const _ApprovalButton({
    required this.option,
    required this.enabled,
    required this.onPressed,
  });

  final ApprovalOption option;
  final bool enabled;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final approve =
        option.kind.contains('allow') || option.kind.contains('approve');
    if (approve) {
      return FilledButton(
        key: Key('approval-option-${option.optionId}'),
        onPressed: enabled ? onPressed : null,
        child: Text(option.name),
      );
    }
    return OutlinedButton(
      key: Key('approval-option-${option.optionId}'),
      onPressed: enabled ? onPressed : null,
      child: Text(option.name),
    );
  }
}

class _InlineNotice extends StatelessWidget {
  const _InlineNotice({required this.notice});

  final _NoticeItem notice;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      liveRegion: notice.isError,
      child: Text(
        notice.text,
        style: Theme.of(context).textTheme.bodySmall?.copyWith(
          color:
              notice.isError
                  ? Theme.of(context).colorScheme.error
                  : palette.muted,
        ),
      ),
    );
  }
}

class _ConnectionNotice extends StatelessWidget {
  const _ConnectionNotice({
    required this.text,
    required this.canRetry,
    required this.onRetry,
    this.onPair,
  });

  final String text;
  final bool canRetry;
  final VoidCallback onRetry;
  final VoidCallback? onPair;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      liveRegion: true,
      child: Padding(
        padding: const EdgeInsets.only(top: 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              text,
              key: const Key('connection-notice'),
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                color: Theme.of(context).colorScheme.error,
              ),
            ),
            if (canRetry || onPair != null)
              Align(
                alignment: AlignmentDirectional.centerEnd,
                child: Wrap(
                  alignment: WrapAlignment.end,
                  spacing: 4,
                  children: [
                    if (canRetry)
                      TextButton(
                        key: const Key('retry-connection'),
                        onPressed: onRetry,
                        child: const Text('重新連線'),
                      ),
                    if (onPair != null)
                      FilledButton.tonalIcon(
                        key: const Key('open-pairing-settings'),
                        onPressed: onPair,
                        icon: const Icon(Icons.link_rounded),
                        label: const Text('設定與配對'),
                      ),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _Composer extends StatelessWidget {
  const _Composer({
    required this.controller,
    required this.focusNode,
    required this.enabled,
    required this.disabledHint,
    required this.sending,
    required this.onSend,
    required this.voiceVisible,
    required this.voiceReady,
    required this.voiceRecording,
    required this.voiceProcessing,
    required this.voiceSummary,
    required this.voiceError,
    required this.onVoiceAction,
    required this.onVoiceCancel,
  });

  final TextEditingController controller;
  final FocusNode focusNode;
  final bool enabled;
  final String disabledHint;
  final bool sending;
  final VoidCallback onSend;
  final bool voiceVisible;
  final bool voiceReady;
  final bool voiceRecording;
  final bool voiceProcessing;
  final String voiceSummary;
  final String? voiceError;
  final VoidCallback onVoiceAction;
  final VoidCallback onVoiceCancel;

  @override
  Widget build(BuildContext context) {
    final voiceBusy = voiceRecording || voiceProcessing;
    final canSend =
        enabled && !sending && !voiceBusy && controller.text.trim().isNotEmpty;
    final canStartVoice = enabled && !sending && voiceReady && !voiceProcessing;
    final colors = Theme.of(context).colorScheme;
    KeyEventResult handleComposerKey(FocusNode node, KeyEvent event) {
      if (!kIsWeb || event is! KeyDownEvent) return KeyEventResult.ignored;
      final isEnter =
          event.logicalKey == LogicalKeyboardKey.enter ||
          event.logicalKey == LogicalKeyboardKey.numpadEnter;
      if (!isEnter || HardwareKeyboard.instance.isShiftPressed || !canSend) {
        return KeyEventResult.ignored;
      }
      onSend();
      return KeyEventResult.handled;
    }

    return Padding(
      padding: const EdgeInsets.fromLTRB(0, 10, 0, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (voiceVisible && (voiceBusy || voiceError != null)) ...[
            Semantics(
              liveRegion: true,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(14, 0, 12, 7),
                child: Row(
                  children: [
                    Icon(
                      voiceRecording
                          ? Icons.fiber_manual_record_rounded
                          : voiceError != null
                          ? Icons.error_outline_rounded
                          : Icons.graphic_eq_rounded,
                      size: 16,
                      color:
                          voiceError != null
                              ? colors.error
                              : voiceRecording
                              ? colors.error
                              : colors.primary,
                    ),
                    const SizedBox(width: 7),
                    Expanded(
                      child: Text(
                        voiceError ??
                            (voiceRecording
                                ? '錄音中 · 點停止後才會開始轉寫'
                                : '正在轉寫 · 完成後會先開啟可編輯草稿'),
                        key: const Key('voice-composer-status'),
                        style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color:
                              voiceError != null
                                  ? colors.error
                                  : _Palette.of(context).muted,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
          Semantics(
            textField: true,
            label: '告訴 Miku',
            child: Focus(
              onKeyEvent: handleComposerKey,
              child: TextField(
                key: const Key('conversation-composer'),
                controller: controller,
                focusNode: focusNode,
                enabled: enabled,
                minLines: 1,
                maxLines: 6,
                textCapitalization: TextCapitalization.sentences,
                keyboardType: TextInputType.multiline,
                textInputAction: TextInputAction.newline,
                decoration: InputDecoration(
                  hintText: enabled ? '告訴 Miku…' : disabledHint,
                  suffixIconConstraints: const BoxConstraints(minHeight: 54),
                  suffixIcon: Padding(
                    padding: const EdgeInsetsDirectional.only(end: 5),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        if (voiceVisible) ...[
                          IconButton(
                            key: const Key('voice-capture-action'),
                            tooltip:
                                voiceRecording
                                    ? '停止錄音並轉寫'
                                    : voiceProcessing
                                    ? '語音正在清理或轉寫'
                                    : voiceReady
                                    ? '開始語音輸入 · $voiceSummary'
                                    : '語音模型尚未就緒，請到設定檢查',
                            constraints: const BoxConstraints.tightFor(
                              width: 44,
                              height: 44,
                            ),
                            onPressed:
                                voiceRecording
                                    ? onVoiceAction
                                    : canStartVoice
                                    ? onVoiceAction
                                    : null,
                            icon:
                                voiceProcessing && !voiceRecording
                                    ? const SizedBox.square(
                                      dimension: 18,
                                      child: CircularProgressIndicator(
                                        strokeWidth: 2,
                                      ),
                                    )
                                    : Icon(
                                      voiceRecording
                                          ? Icons.stop_rounded
                                          : Icons.mic_none_rounded,
                                    ),
                          ),
                          if (voiceBusy)
                            IconButton(
                              key: const Key('voice-capture-cancel'),
                              tooltip: '取消語音輸入並清除錄音',
                              constraints: const BoxConstraints.tightFor(
                                width: 44,
                                height: 44,
                              ),
                              onPressed: onVoiceCancel,
                              icon: const Icon(Icons.close_rounded),
                            ),
                        ],
                        IconButton.filled(
                          key: const Key('send-message'),
                          tooltip: '送出',
                          constraints: const BoxConstraints.tightFor(
                            width: 44,
                            height: 44,
                          ),
                          onPressed: canSend ? onSend : null,
                          style: IconButton.styleFrom(
                            backgroundColor: colors.primary,
                            foregroundColor: colors.onPrimary,
                            disabledBackgroundColor: colors.onSurface
                                .withValues(alpha: 0.12),
                            disabledForegroundColor: colors.onSurface
                                .withValues(alpha: 0.38),
                          ),
                          icon:
                              sending
                                  ? const SizedBox.square(
                                    dimension: 18,
                                    child: CircularProgressIndicator(
                                      strokeWidth: 2,
                                    ),
                                  )
                                  : const Icon(Icons.arrow_upward_rounded),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _QuietLoading extends StatelessWidget {
  const _QuietLoading();

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      liveRegion: true,
      label: '正在載入對話',
      child: SizedBox.square(
        dimension: 24,
        child: CircularProgressIndicator(strokeWidth: 2, color: palette.miku),
      ),
    );
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

String _fallbackOptionName(String kind) {
  if (kind.contains('allow') || kind.contains('approve')) return '允許一次';
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
