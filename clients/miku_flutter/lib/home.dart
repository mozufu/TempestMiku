part of 'main.dart';

class MikuHomePage extends StatefulWidget {
  const MikuHomePage({
    super.key,
    required this.client,
    required this.notifications,
    required this.shareImports,
  });

  final MikuSessionClient client;
  final MikuNotificationService notifications;
  final MikuShareImportService shareImports;

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
  int _sendEpoch = 0;
  _AppDestination _destination = _AppDestination.chat;
  _UiLanguage _language = _UiLanguage.en;
  AppLifecycleState _appLifecycle = AppLifecycleState.resumed;

  late final AnimationController _dotAnim;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _dotAnim = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    )..repeat();
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
    unawaited(_boot());
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
    _dotAnim.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _appLifecycle = state;
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
    _sessionBootComplete = true;
    await _drainNotificationActions();
    await _drainNotificationRoutes();
    await _drainShareImports();
  }

  void _enqueueShareImport(SharedContent content) {
    if (content.source == SharedContentSource.quickCapture) {
      final eventId = content.eventId;
      if (eventId == null || _recentQuickCaptureIds.contains(eventId)) return;
      _recentQuickCaptureIds.add(eventId);
      if (_recentQuickCaptureIds.length > 64) {
        _recentQuickCaptureIds.removeAt(0);
      }
      final active = _activeShareImport;
      if (active?.value.source == SharedContentSource.quickCapture) {
        active!.value = content;
        return;
      }
      _pendingShareImports.removeWhere(
        (pending) => pending.source == SharedContentSource.quickCapture,
      );
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

  Future<bool> _applyPairingLink(String rawLink) async {
    final client = _serverTargetClient;
    if (client == null) return false;
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
      await client.pairWithCode(target);
      await _requestApprovalNotifications();
      final pushNotifications = _unifiedPushNotifications;
      if (pushNotifications != null) {
        await _initializeUnifiedPush(pushNotifications);
      }
      await _reconnectAfterPair(
        successMessage: _copy.pairedToServer(target.serverBaseUrl),
      );
      return true;
    } catch (err) {
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

  _ActivityItem? _activityFromEvent(MikuEvent e) {
    final data = e.data;
    switch (e.type) {
      case 'tool_call':
        final name = _eventText(data, 'name', fallback: 'execute');
        return _ActivityItem(
          icon: Icons.build_outlined,
          title: '呼叫工具 $name',
          detail: _eventText(data, 'arguments'),
          state: _ActivityState.running,
          monospace: true,
          kind: 'tool',
        );
      case 'tool_call_update':
        final name = _eventText(data, 'name', fallback: 'execute');
        return _ActivityItem(
          icon: Icons.more_horiz,
          title: '更新工具參數 $name',
          detail: _eventText(data, 'arguments'),
          state: _ActivityState.running,
          monospace: true,
          kind: 'tool',
        );
      case 'cell_start':
        return _ActivityItem(
          icon: Icons.terminal,
          title: '執行程式',
          detail: _eventText(data, 'code'),
          state: _ActivityState.running,
          monospace: true,
          kind: 'cell',
        );
      case 'cell_result':
        final shaped = _eventText(data, 'shaped');
        return _ActivityItem(
          icon:
              shaped.startsWith('error:')
                  ? Icons.error_outline
                  : Icons.check_circle_outline,
          title: shaped.startsWith('error:') ? '程式失敗' : '程式結果',
          detail: shaped,
          state:
              shaped.startsWith('error:')
                  ? _ActivityState.failed
                  : _ActivityState.done,
          monospace: true,
          kind: 'cell',
          resourceUris: _extractResources(shaped),
        );
      case 'actor_spawned':
        final actorId = _eventText(
          data,
          'actor_id',
          camelKey: 'actorId',
          fallback: _eventText(data, 'id'),
        );
        final role = _eventText(data, 'role', fallback: 'worker');
        return _ActivityItem(
          icon: Icons.account_tree_outlined,
          title: '啟動 $role · $actorId',
          detail: _eventText(data, 'task'),
          state: _ActivityState.running,
          kind: 'actor',
          actorId: actorId,
          role: role,
        );
      case 'actor_status':
        final actorId = _eventText(
          data,
          'actor_id',
          camelKey: 'actorId',
          fallback: _eventText(data, 'id'),
        );
        final status = _eventText(data, 'status', fallback: 'updated');
        return _ActivityItem(
          icon: Icons.timeline,
          title: '$actorId 狀態 $status',
          detail: '',
          state:
              status == 'terminated'
                  ? _ActivityState.done
                  : _ActivityState.running,
          kind: 'actor',
          actorId: actorId,
        );
      case 'actor_message':
        final actorId = _eventText(
          data,
          'actor_id',
          camelKey: 'actorId',
          fallback: _eventText(data, 'from'),
        );
        return _ActivityItem(
          icon: Icons.chat_bubble_outline,
          title: '$actorId 訊息',
          detail: _eventText(
            data,
            'text',
            fallback: _eventText(data, 'message'),
          ),
          state: _ActivityState.info,
          kind: 'actor',
          actorId: actorId,
        );
      case 'actor_completed':
        final actorId = _eventText(
          data,
          'actor_id',
          camelKey: 'actorId',
          fallback: _eventText(data, 'id'),
        );
        final summary = _eventText(data, 'summary');
        final resources =
            [
              _eventText(data, 'artifact_uri', camelKey: 'artifactUri'),
              _eventText(data, 'history_uri', camelKey: 'historyUri'),
            ].where((uri) => uri.isNotEmpty).toList();
        return _ActivityItem(
          icon: Icons.task_alt,
          title: '完成 $actorId',
          detail: summary,
          state: _ActivityState.done,
          kind: 'actor',
          actorId: actorId,
          resourceUris: resources,
        );
      case 'actor_failed':
        final actorId = _eventText(
          data,
          'actor_id',
          camelKey: 'actorId',
          fallback: _eventText(data, 'id'),
        );
        return _ActivityItem(
          icon: Icons.error_outline,
          title: '$actorId 失敗',
          detail: _eventText(
            data,
            'error',
            fallback: _eventText(
              data,
              'failure_reason',
              camelKey: 'failureReason',
            ),
          ),
          state: _ActivityState.failed,
          kind: 'actor',
          actorId: actorId,
        );
      case 'actor_cancelled':
        final actorId = _eventText(
          data,
          'actor_id',
          camelKey: 'actorId',
          fallback: _eventText(data, 'id'),
        );
        return _ActivityItem(
          icon: Icons.cancel_outlined,
          title: '取消 $actorId',
          detail: _eventText(data, 'reason'),
          state: _ActivityState.failed,
          kind: 'actor',
          actorId: actorId,
        );
      case 'write_proposal':
        final review = EvolutionReviewProposal.fromEvent(data);
        if (review != null) {
          return _ActivityItem(
            icon: Icons.fact_check_outlined,
            title:
                '${review.targetKind} addendum · ${review.targetId} · ${review.status}',
            detail: _joinedDetail([
              review.preview,
              review.applyEnabled
                  ? 'Apply enabled'
                  : 'Review only · apply disabled',
            ]),
            state: switch (review.status) {
              'approved' => _ActivityState.done,
              'denied' || 'timed_out' || 'cancelled' => _ActivityState.failed,
              _ => _ActivityState.info,
            },
            kind: 'evolution_review',
            resourceUris:
                review.resourceUri.isEmpty ? const [] : [review.resourceUri],
          );
        }
        if (_eventText(data, 'kind') != 'drive') return null;
        final preview = _eventMap(data['preview']);
        return _ActivityItem(
          icon: Icons.rule_folder_outlined,
          title: _eventText(
            preview ?? const <String, Object?>{},
            'title',
            fallback: 'Drive organizer proposal',
          ),
          detail: _joinedDetail([
            _eventText(preview ?? const <String, Object?>{}, 'subtitle'),
            _eventText(preview ?? const <String, Object?>{}, 'snippet'),
          ]),
          state: _ActivityState.info,
          kind: 'drive',
          resourceUris: _resourceUrisFromEvent(data),
        );
      case 'drive_put':
      case 'drive_moved':
      case 'drive_tagged':
      case 'drive_linked':
      case 'drive_unlinked':
        return _driveActivityFromEvent(e);
      case 'drive_organizer_started':
        final tier = _eventText(data, 'tier', fallback: 'conservative');
        final apply = data['apply'] == true ? 'apply' : 'propose';
        return _ActivityItem(
          icon: Icons.rule_folder_outlined,
          title: 'Drive organizer started',
          detail: '$tier · $apply',
          state: _ActivityState.running,
          kind: 'drive',
        );
      case 'drive_organizer_completed':
        return _ActivityItem(
          icon: Icons.task_alt,
          title: 'Drive organizer completed',
          detail: _driveOrganizerDetail(data),
          state: _ActivityState.done,
          kind: 'drive',
          resourceUris: _resourceUrisFromEvent(data),
        );
      case 'drive_organizer_failed':
        return _ActivityItem(
          icon: Icons.error_outline,
          title: 'Drive organizer failed',
          detail: _eventText(
            data,
            'error',
            fallback: _driveOrganizerDetail(data),
          ),
          state: _ActivityState.failed,
          kind: 'drive',
          resourceUris: _resourceUrisFromEvent(data),
        );
    }
    return null;
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

  List<String> _extractResources(String text) {
    return RegExp(
          r'''\b(?:artifact|workspace|linked|project|drive)://[^\s),\]\}"']+''',
        )
        .allMatches(text)
        .map((m) => _normalizeResourceUri(m.group(0)!))
        .toSet()
        .toList();
  }

  List<String> _promotionResources(String finalText) {
    final resources = <String>[];
    void add(String uri) {
      final normalized = _normalizeResourceUri(uri);
      if (normalized.isEmpty || resources.contains(normalized)) return;
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

  String _normalizeResourceUri(String uri) {
    return _cleanResourceUri(uri);
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
          (sheetContext) => ConstrainedBox(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.of(sheetContext).size.height * 0.9,
            ),
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

  void _showOverflowSheet() {
    final tok = _tok;
    final serverTargetClient = _serverTargetClient;
    final themeController = MikuThemeScope.controllerOf(context);
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      isScrollControlled: true,
      useSafeArea: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(20)),
      ),
      builder:
          (sheetContext) => _OverflowSheet(
            tok: tok,
            copy: _copy,
            projectStatus: _projectStatus,
            nextActions: _nextActions,
            themeMode: themeController.mode,
            onRefresh: () {
              Navigator.pop(sheetContext);
              unawaited(_loadProject());
            },
            onPromote: () {
              Navigator.pop(sheetContext);
              unawaited(_promoteSession());
            },
            onDrive: () {
              Navigator.pop(sheetContext);
              Timer(const Duration(milliseconds: 320), () {
                if (mounted) _showDriveSheet();
              });
            },
            onThemeModeChanged:
                (mode) => unawaited(themeController.setMode(mode)),
            onLanguageToggle: () => unawaited(_toggleLanguage()),
            onModeSettings: () {
              Navigator.pop(sheetContext);
              Timer(const Duration(milliseconds: 320), () {
                if (mounted) _showModeSheet();
              });
            },
            onServerTarget:
                serverTargetClient == null
                    ? null
                    : () {
                      Navigator.pop(sheetContext);
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
                      Navigator.pop(sheetContext);
                      unawaited(_disconnectFromServer(serverTargetClient));
                    },
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
      });
    }
    final future = _connectSession(navigationEpoch);
    _sessionFuture = future;
    await future;
    if (successMessage != null &&
        mounted &&
        navigationEpoch == _sessionNavigationEpoch) {
      _showSnack(successMessage);
    }
  }

  void _selectDestination(_AppDestination destination) {
    if (_destination == destination) return;
    setState(() => _destination = destination);
    if (destination == _AppDestination.drive) {
      unawaited(_loadDriveFeed(silent: true));
    }
  }

  void _startFreshChat() {
    if (_disconnecting) return;
    if (_destination != _AppDestination.chat) {
      setState(() => _destination = _AppDestination.chat);
    }
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
    final navigationEpoch = _nextSessionNavigationEpoch();
    _disconnecting = true;
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

    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: isDark ? SystemUiOverlayStyle.light : SystemUiOverlayStyle.dark,
      child: Scaffold(
        backgroundColor: tok.bg,
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
                    : LayoutBuilder(
                      builder:
                          (context, constraints) =>
                              _buildAdaptiveShell(constraints, tok, accent),
                    ),
          ),
        ),
      ),
    );
  }

  Widget _buildAdaptiveShell(
    BoxConstraints constraints,
    _Tok tok,
    Color accent,
  ) {
    if (constraints.maxWidth < 600) {
      return Column(
        children: [
          _buildTopBar(tok, compact: true, showBrand: true),
          _buildConnectionBanner(tok),
          Expanded(child: _buildPrimaryDestination(tok, accent)),
          if (_destination == _AppDestination.chat) ...[
            _ApprovalAttentionBar(
              tok: tok,
              copy: _copy,
              approvals: _approvals,
              onOpen: _showApprovalSheet,
            ),
            _buildComposer(tok, accent),
          ],
          _MikuBottomNavigation(
            destination: _destination,
            copy: _copy,
            onSelected: _selectDestination,
          ),
        ],
      );
    }

    final rail = _MikuNavigationRail(
      destination: _destination,
      copy: _copy,
      onSelected: _selectDestination,
      brand: const MikuBrandBadge(size: 46),
      onSettings: _showOverflowSheet,
    );

    if (constraints.maxWidth < 1100 || _destination != _AppDestination.chat) {
      return Row(
        children: [
          rail,
          VerticalDivider(width: 1, color: tok.border),
          Expanded(
            child: Column(
              children: [
                _buildTopBar(tok, compact: false, showBrand: false),
                _buildConnectionBanner(tok),
                Expanded(child: _buildPrimaryDestination(tok, accent)),
                if (_destination == _AppDestination.chat) ...[
                  _ApprovalAttentionBar(
                    tok: tok,
                    copy: _copy,
                    approvals: _approvals,
                    onOpen: _showApprovalSheet,
                  ),
                  _buildComposer(tok, accent),
                ],
              ],
            ),
          ),
        ],
      );
    }

    final sessionPaneWidth = constraints.maxWidth >= 1320 ? 280.0 : 220.0;
    final contextPaneWidth = constraints.maxWidth >= 1320 ? 300.0 : 248.0;
    return Row(
      children: [
        rail,
        VerticalDivider(width: 1, color: tok.border),
        SizedBox(
          width: sessionPaneWidth,
          child: ColoredBox(
            color: tok.surface,
            child: _buildSessionsSurface(tok),
          ),
        ),
        VerticalDivider(width: 1, color: tok.border),
        Expanded(
          child: Column(
            children: [
              _buildTopBar(tok, compact: false, showBrand: false),
              _buildConnectionBanner(tok),
              Expanded(
                child: _buildChatSurface(
                  tok,
                  accent,
                  showPendingApprovals: false,
                ),
              ),
              _buildComposer(tok, accent),
            ],
          ),
        ),
        VerticalDivider(width: 1, color: tok.border),
        SizedBox(
          width: contextPaneWidth,
          child: _MikuContextPanel(
            tok: tok,
            copy: _copy,
            accent: accent,
            projectStatus: _projectStatus,
            nextActions: _nextActions,
            approvals: _approvals,
            onOpenApproval: _showApprovalSheet,
            onPromote: _promoteSession,
            onRefresh: _loadProject,
          ),
        ),
      ],
    );
  }

  Widget _buildTopBar(
    _Tok tok, {
    required bool compact,
    required bool showBrand,
  }) {
    final copy = _copy;
    final online =
        _status == 'connected' ||
        _status == 'streaming' ||
        _status == 'complete';
    final statusColor =
        online
            ? tok.success
            : _status == 'connecting'
            ? tok.cool
            : tok.warning;
    return Container(
      constraints: const BoxConstraints(minHeight: 64),
      padding: EdgeInsets.fromLTRB(compact ? 14 : 18, 8, 10, 8),
      decoration: BoxDecoration(
        color: tok.glass,
        border: Border(bottom: BorderSide(color: tok.glassBorder)),
      ),
      child: Row(
        children: [
          if (showBrand) ...[
            const MikuBrandBadge(size: 40),
            const SizedBox(width: 11),
          ],
          Expanded(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  compact ? 'Miku' : 'Tempest Miku',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: compact ? 17 : 18,
                    fontWeight: FontWeight.w900,
                    letterSpacing: -0.35,
                  ),
                ),
                const SizedBox(height: 2),
                Row(
                  children: [
                    Container(
                      width: 7,
                      height: 7,
                      decoration: BoxDecoration(
                        color: statusColor,
                        shape: BoxShape.circle,
                      ),
                    ),
                    const SizedBox(width: 6),
                    Flexible(
                      child: Text(
                        copy.statusLabel(_status),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
          IconButton(
            tooltip: copy.newSession,
            onPressed: _startFreshChat,
            icon: const Icon(Icons.edit_square),
          ),
          IconButton(
            tooltip: copy.pick('Settings', '設定'),
            onPressed: _showOverflowSheet,
            icon: const Icon(Icons.tune_rounded),
          ),
        ],
      ),
    );
  }

  Widget _buildConnectionBanner(_Tok tok) {
    return _ConnectionBanner(
      tok: tok,
      copy: _copy,
      status: _status,
      onRetry: _retryConnection,
      onNewSession: _startFreshChat,
    );
  }

  Widget _buildPrimaryDestination(_Tok tok, Color accent) {
    return switch (_destination) {
      _AppDestination.chat => _buildChatSurface(tok, accent),
      _AppDestination.sessions => _buildSessionsSurface(tok),
      _AppDestination.drive => _buildDriveSurface(tok, accent),
    };
  }

  Widget _buildChatSurface(
    _Tok tok,
    Color accent, {
    bool showPendingApprovals = true,
  }) {
    return Stack(
      children: [
        Positioned.fill(
          child: _buildThread(
            tok,
            accent,
            showPendingApprovals: showPendingApprovals,
          ),
        ),
        if (_showJumpToLatest)
          Positioned(
            right: 18,
            bottom: 12,
            child: Semantics(
              button: true,
              label: _copy.pick('Jump to latest message', '跳到最新訊息'),
              child: FloatingActionButton.small(
                heroTag: 'jump-to-latest',
                onPressed: () => _scrollToBottom(force: true, animate: true),
                child: const Icon(Icons.arrow_downward_rounded),
              ),
            ),
          ),
      ],
    );
  }

  Widget _buildSessionsSurface(_Tok tok) {
    return _SessionHistorySheet(
      tok: tok,
      copy: _copy,
      currentSessionId: _sessionId,
      loadSessions: widget.client.listSessions,
      embedded: true,
      refreshToken: _sessionHistoryRevision,
      onSelect: (sessionId) {
        setState(() => _destination = _AppDestination.chat);
        unawaited(_loadHistoricalSession(sessionId));
      },
      onNewSession: _startFreshChat,
    );
  }

  Widget _buildDriveSurface(_Tok tok, Color accent) {
    return _DriveFeedSheet(
      tok: tok,
      copy: _copy,
      accent: accent,
      initialFeed: _driveFeed,
      initialError: _driveError,
      initialLoading: _driveLoading,
      approvals: _driveApprovals,
      loadFeed: _fetchDriveFeed,
      onOpenResource: _openResource,
      onOpenApproval: _showApprovalSheet,
      embedded: true,
    );
  }

  Widget _buildThread(
    _Tok tok,
    Color accent, {
    bool showPendingApprovals = true,
  }) {
    final copy = _copy;
    final items = <Widget>[];

    if (_rounds.isEmpty && _memoryProposals.isEmpty && _approvals.isEmpty) {
      items.add(_EmptyState(tok: tok, status: _status, copy: copy));
      items.add(const SizedBox(height: 14));
    }

    for (final round in _rounds) {
      if (round.userText.isNotEmpty) {
        items.add(
          _UserBubble(tok: tok, text: round.userText, accent: tok.accentSoft),
        );
        items.add(const SizedBox(height: 10));
      }

      final assistantText = round.assistantText;
      void addActivityTrace() {
        if (round.activities.isEmpty) return;
        items.add(
          _AgentStatusBar(
            tok: tok,
            copy: copy,
            accent: accent,
            anim: _dotAnim,
            roundIndex: round.index,
            agents: _agentStatuses(round.activities),
            activities: round.activities,
            expanded: round.activityExpanded,
            onTap: () => _showActivitySheet(round),
            onOpenResource: _openResource,
          ),
        );
        items.add(const SizedBox(height: 10));
      }

      void addReasoningTrace() {
        if (!round.hasReasoning) return;
        items.add(
          _ThinkingTrace(
            tok: tok,
            copy: copy,
            accent: accent,
            text: round.reasoningText,
            expanded: round.reasoningExpanded,
            isStreaming:
                round.assistantFinalText.isEmpty &&
                round.isStreaming &&
                round.assistantStreamedText.isEmpty,
          ),
        );
        items.add(const SizedBox(height: 10));
      }

      void addAssistantAnswer() {
        if (assistantText.isEmpty) return;
        final resources = _extractResources(assistantText);
        items.add(
          _MikuBubble(
            tok: tok,
            copy: copy,
            text: assistantText,
            accent: accent,
            resources: resources,
            onOpenResource: _openResource,
            isStreaming: round.assistantFinalText.isEmpty && round.isStreaming,
          ),
        );
      }

      if (assistantText.isNotEmpty) {
        addAssistantAnswer();
        items.add(const SizedBox(height: 10));
      } else if (round.isStreaming) {
        items.add(_TypingIndicator(tok: tok, accent: accent, anim: _dotAnim));
        items.add(const SizedBox(height: 10));
      }
      addActivityTrace();
      addReasoningTrace();
      items.add(const SizedBox(height: 14));
    }

    for (final proposal in _memoryProposals) {
      final approval = _approvalForProposal(proposal);
      items.add(
        _MemoryProposalCard(
          tok: tok,
          copy: copy,
          proposal: proposal,
          approval: approval,
          accent: accent,
          onApprove:
              approval == null ? null : () => _resolve(approval, 'approve'),
          onDeny: approval == null ? null : () => _resolve(approval, 'deny'),
        ),
      );
      items.add(const SizedBox(height: 10));
    }

    if (showPendingApprovals) {
      for (final approval in _approvals.where(
        (item) => !_isRenderedAsMemoryProposal(item),
      )) {
        items.add(
          _ApprovalCard(
            tok: tok,
            copy: copy,
            approval: approval,
            accent: accent,
            onTap: () => _showApprovalSheet(approval),
          ),
        );
        items.add(const SizedBox(height: 10));
      }
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        return Center(
          child: SizedBox(
            width: math.min(constraints.maxWidth, 720),
            height: constraints.maxHeight,
            child: ListView.builder(
              controller: _scrollCtrl,
              padding: const EdgeInsets.fromLTRB(14, 10, 14, 16),
              itemCount: items.length,
              itemBuilder: (context, index) => items[index],
            ),
          ),
        );
      },
    );
  }

  Widget _buildComposer(_Tok tok, Color accent) {
    final copy = _copy;
    final canSubmit = _canSend && !_sessionEnded && !_isSending;
    return LayoutBuilder(
      builder: (context, constraints) {
        return Container(
          padding: const EdgeInsets.fromLTRB(14, 8, 14, 12),
          decoration: BoxDecoration(
            color: tok.glass,
            border: Border(top: BorderSide(color: tok.glassBorder)),
          ),
          child: Center(
            child: SizedBox(
              width: math.min(constraints.maxWidth, 720),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (_sendError.isNotEmpty) ...[
                    Semantics(
                      liveRegion: true,
                      label: _sendError,
                      child: Container(
                        width: double.infinity,
                        margin: const EdgeInsets.only(bottom: 8),
                        padding: const EdgeInsets.symmetric(
                          horizontal: 12,
                          vertical: 9,
                        ),
                        decoration: BoxDecoration(
                          color: tok.danger.withValues(alpha: 0.1),
                          borderRadius: BorderRadius.circular(14),
                          border: Border.all(
                            color: tok.danger.withValues(alpha: 0.45),
                          ),
                        ),
                        child: Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Icon(
                              Icons.error_outline_rounded,
                              size: 19,
                              color: tok.danger,
                            ),
                            const SizedBox(width: 8),
                            Expanded(
                              child: Text(
                                _sendError,
                                style: TextStyle(
                                  color: tok.text,
                                  fontSize: 12.5,
                                  height: 1.35,
                                  fontWeight: FontWeight.w600,
                                ),
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ],
                  Container(
                    decoration: BoxDecoration(
                      color: tok.raised,
                      border: Border.all(
                        color:
                            _sendError.isEmpty
                                ? tok.border
                                : tok.danger.withValues(alpha: 0.7),
                      ),
                      borderRadius: BorderRadius.circular(20),
                      boxShadow: [
                        BoxShadow(
                          color: tok.glow,
                          blurRadius: 18,
                          offset: const Offset(0, 8),
                        ),
                      ],
                    ),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.end,
                      children: [
                        Expanded(
                          child: TextField(
                            controller: _inputCtrl,
                            enabled: !_sessionEnded,
                            readOnly: _isSending,
                            style: TextStyle(
                              color: tok.text,
                              fontSize: 15,
                              height: 1.4,
                            ),
                            minLines: 1,
                            maxLines: 6,
                            keyboardType: TextInputType.multiline,
                            textInputAction: TextInputAction.send,
                            decoration: InputDecoration(
                              hintText:
                                  _sessionEnded
                                      ? copy.sessionEndedHint
                                      : copy.messageHint,
                              filled: false,
                              border: InputBorder.none,
                              enabledBorder: InputBorder.none,
                              focusedBorder: InputBorder.none,
                              contentPadding: const EdgeInsets.fromLTRB(
                                16,
                                14,
                                8,
                                14,
                              ),
                            ),
                            onChanged: (value) {
                              final text = value.trim();
                              final shouldSend =
                                  !_sessionEnded && text.isNotEmpty;
                              final changedPending =
                                  _pendingMessageText != null &&
                                  _pendingMessageText != text;
                              if (shouldSend != _canSend ||
                                  changedPending ||
                                  _sendError.isNotEmpty) {
                                setState(() {
                                  _canSend = shouldSend;
                                  if (changedPending) {
                                    final optimisticRound =
                                        _pendingOptimisticRound;
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
                            },
                            onSubmitted: (_) {
                              if (canSubmit) unawaited(_send());
                            },
                          ),
                        ),
                        Padding(
                          padding: const EdgeInsets.all(6),
                          child: Semantics(
                            button: true,
                            enabled: canSubmit,
                            label: copy.sendMessage,
                            child: Tooltip(
                              message:
                                  _sessionEnded
                                      ? copy.sessionEnded
                                      : canSubmit
                                      ? copy.send
                                      : copy.typeMessage,
                              child: SizedBox.square(
                                dimension: 48,
                                child: IconButton.filled(
                                  onPressed: canSubmit ? _send : null,
                                  style: IconButton.styleFrom(
                                    backgroundColor: accent,
                                    foregroundColor: _textOn(accent),
                                    disabledBackgroundColor: tok.border
                                        .withValues(alpha: 0.55),
                                    disabledForegroundColor: tok.muted,
                                    shape: RoundedRectangleBorder(
                                      borderRadius: BorderRadius.circular(16),
                                    ),
                                  ),
                                  icon:
                                      _isSending
                                          ? SizedBox.square(
                                            dimension: 20,
                                            child: CircularProgressIndicator(
                                              strokeWidth: 2.2,
                                              color: _textOn(accent),
                                            ),
                                          )
                                          : const Icon(Icons.send, size: 21),
                                ),
                              ),
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}
