part of 'main.dart';

// ─── Conversation round model ──────────────────────────────────────────────────

class _ConversationRound {
  _ConversationRound({
    required this.index,
    required this.userText,
    this.isStreaming = true,
  });

  final int index;
  final String userText;
  final List<_ActivityItem> activities = [];
  String assistantStreamedText = '';
  String assistantFinalText = '';
  bool isStreaming;
  bool activityExpanded = false;

  String get assistantText => assistantFinalText.isNotEmpty
      ? assistantFinalText
      : assistantStreamedText;

  bool get isComplete => assistantFinalText.isNotEmpty && !isStreaming;
}

enum _ActivityState { running, done, failed, info }

class _ActivityItem {
  const _ActivityItem({
    required this.icon,
    required this.title,
    required this.detail,
    required this.state,
    this.monospace = false,
    this.kind = 'event',
    this.actorId,
    this.role,
    this.resourceUris = const [],
  });

  final IconData icon;
  final String title;
  final String detail;
  final _ActivityState state;
  final bool monospace;
  final String kind;
  final String? actorId;
  final String? role;
  final List<String> resourceUris;
}

class _AgentStatus {
  const _AgentStatus({
    required this.id,
    required this.role,
    required this.state,
    required this.detail,
  });

  final String id;
  final String role;
  final _ActivityState state;
  final String detail;

  bool get isRunning => state == _ActivityState.running;
}

// ─── Home page ─────────────────────────────────────────────────────────────────

class MikuHomePage extends StatefulWidget {
  const MikuHomePage({super.key, required this.client});

  final MikuSessionClient client;

  @override
  State<MikuHomePage> createState() => _MikuHomePageState();
}

class _MikuHomePageState extends State<MikuHomePage>
    with SingleTickerProviderStateMixin {
  final _inputCtrl = TextEditingController();
  final _scrollCtrl = ScrollController();
  final List<ApprovalPrompt> _approvals = [];
  final List<MemoryWriteProposal> _memoryProposals = [];
  final List<String> _nextActions = [];
  final List<_ConversationRound> _rounds = [];
  final List<_Mode> _modes = [];

  Future<void>? _sessionFuture;
  StreamSubscription<MikuEvent>? _sub;
  String? _sessionId;
  String? _lastEventId;
  String _modeId = '';
  String _defaultModeId = '';
  String _status = 'idle';
  String _projectStatus = '';
  bool _isDark = true;
  bool _modeLocked = false;
  bool _canSend = false;
  _UiLanguage _language = _UiLanguage.en;

  late final AnimationController _dotAnim;

  @override
  void initState() {
    super.initState();
    _dotAnim = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    )..repeat();
    unawaited(_ensureSession());
  }

  @override
  void dispose() {
    _inputCtrl.dispose();
    _scrollCtrl.dispose();
    _sub?.cancel();
    _dotAnim.dispose();
    super.dispose();
  }

  _Mode get _mode => _findMode(_modeId, _modes);
  _Tok get _tok => _isDark ? _Tok.dark : _Tok.light;
  Color get _accent => _tok.accentSoft;
  _UiCopy get _copy => _UiCopy(_language);

  // ── Session ────────────────────────────────────────────────────────────────

  Future<void> _ensureSession() async {
    if (_sessionId != null) return;
    return _sessionFuture ??= _connectSession();
  }

  Future<void> _connectSession() async {
    if (mounted) setState(() => _status = 'connecting');
    try {
      await _loadModes();
      final s = await widget.client.createOrReuseSession();
      LoadedSession? loaded;
      try {
        loaded = await widget.client.loadSession(s.id);
      } catch (_) {
        loaded = null;
      }
      await _attachSession(
        loaded?.session ?? s,
        messages: loaded?.messages ?? const [],
        pendingEvents: loaded?.pendingEvents ?? const [],
      );
    } catch (err) {
      _sessionFuture = null;
      if (!mounted) return;
      setState(() => _status = 'offline');
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not connect to tm-server: $err')),
      );
    }
  }

  Future<void> _attachSession(
    MikuSession session, {
    List<SessionMessage> messages = const [],
    List<MikuEvent> pendingEvents = const [],
  }) async {
    final previousSub = _sub;
    _sub = null;
    if (!mounted) return;
    setState(() {
      _mergeSessionMode(session);
      _sessionId = session.id;
      _lastEventId = session.lastEventId;
      _modeId = session.mode.isEmpty ? _defaultModeId : session.mode;
      _modeLocked = session.locked;
      _status = 'connected';
      _approvals.clear();
      _memoryProposals.clear();
      _rounds
        ..clear()
        ..addAll(_roundsFromMessages(messages));
      for (final event in pendingEvents) {
        _applyEvent(event);
      }
    });
    await previousSub?.cancel();
    _sub = widget.client
        .events(session.id, lastEventId: _lastEventId)
        .listen(_onEvent, onError: (_) {
      if (mounted) setState(() => _status = 'reconnecting');
    });
    await _loadProject();
    if (mounted) setState(() {});
    _scrollToBottom();
  }

  Future<void> _loadModes() async {
    final catalog = await widget.client.modeCatalog();
    if (!mounted) return;
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
          capabilityClass: session.defaultScope.startsWith('project:')
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
      final round = rounds.isNotEmpty && rounds.last.assistantFinalText.isEmpty
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

  void _onEvent(MikuEvent e) {
    _rememberEventCursor(e);
    setState(() => _applyEvent(e));
    _scrollToBottom();
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
        round.activityExpanded = false;
        _status = 'connected';
        _loadProject();
      case 'mode':
        final newId = e.data['mode'] as String? ?? _modeId;
        _mergeSessionMode(
          MikuSession(
            id: _sessionId ?? '',
            mode: newId,
            label: e.data['label'] as String? ?? newId,
            voiceCap: (e.data['voice_cap'] as String?) ??
                (e.data['voiceCap'] as String?) ??
                'medium',
            defaultScope: (e.data['defaultScope'] as String?) ??
                (e.data['default_scope'] as String?) ??
                'global',
            activeSkills: ((e.data['activeSkills'] as List?) ?? const [])
                .map((skill) => skill.toString())
                .toList(),
          ),
        );
        _modeLocked = (e.data['locked'] as bool?) ??
            (e.data['lockSource'] != null || e.data['lock_source'] != null);
        _modeId = newId;
      case 'approval':
        final approval = ApprovalPrompt(
          approvalId: e.data['approvalId'] as String? ?? '',
          action: e.data['action'] as String? ?? 'Approval requested',
          scope: (e.data['scope'] as Map?)?.cast<String, Object?>() ?? const {},
          backend: e.data['backend'] as String? ?? '',
          options: ((e.data['options'] as List?) ?? const [])
              .whereType<Map>()
              .map(
                (option) => ApprovalOption(
                  optionId: (option['optionId'] as String?) ??
                      (option['option_id'] as String?) ??
                      '',
                  name: (option['name'] as String?) ?? '',
                  kind: (option['kind'] as String?) ?? '',
                ),
              )
              .where((option) => option.optionId.isNotEmpty)
              .toList(),
          timeoutMs: (e.data['timeoutMs'] as num?)?.toInt() ??
              (e.data['timeout_ms'] as num?)?.toInt(),
        );
        _upsertApproval(approval);
        final proposal = MemoryWriteProposal.fromApproval(approval);
        if (proposal != null) {
          _upsertMemoryProposal(proposal, onlyIfMissing: true);
        }
      case 'approval_resolved':
        _approvals.removeWhere((a) => a.approvalId == e.data['approvalId']);
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
          icon: shaped.startsWith('error:')
              ? Icons.error_outline
              : Icons.check_circle_outline,
          title: shaped.startsWith('error:') ? '程式失敗' : '程式結果',
          detail: shaped,
          state: shaped.startsWith('error:')
              ? _ActivityState.failed
              : _ActivityState.done,
          monospace: true,
          kind: 'cell',
        );
      case 'actor_spawned':
        final actorId = _eventText(data, 'actor_id',
            camelKey: 'actorId', fallback: _eventText(data, 'id'));
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
        final actorId = _eventText(data, 'actor_id',
            camelKey: 'actorId', fallback: _eventText(data, 'id'));
        final status = _eventText(data, 'status', fallback: 'updated');
        return _ActivityItem(
          icon: Icons.timeline,
          title: '$actorId 狀態 $status',
          detail: '',
          state: status == 'terminated'
              ? _ActivityState.done
              : _ActivityState.running,
          kind: 'actor',
          actorId: actorId,
        );
      case 'actor_message':
        final actorId = _eventText(data, 'actor_id',
            camelKey: 'actorId', fallback: _eventText(data, 'from'));
        return _ActivityItem(
          icon: Icons.chat_bubble_outline,
          title: '$actorId 訊息',
          detail:
              _eventText(data, 'text', fallback: _eventText(data, 'message')),
          state: _ActivityState.info,
          kind: 'actor',
          actorId: actorId,
        );
      case 'actor_completed':
        final actorId = _eventText(data, 'actor_id',
            camelKey: 'actorId', fallback: _eventText(data, 'id'));
        final summary = _eventText(data, 'summary');
        final resources = [
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
        final actorId = _eventText(data, 'actor_id',
            camelKey: 'actorId', fallback: _eventText(data, 'id'));
        return _ActivityItem(
          icon: Icons.error_outline,
          title: '$actorId 失敗',
          detail: _eventText(data, 'error',
              fallback: _eventText(data, 'failure_reason',
                  camelKey: 'failureReason')),
          state: _ActivityState.failed,
          kind: 'actor',
          actorId: actorId,
        );
      case 'actor_cancelled':
        final actorId = _eventText(data, 'actor_id',
            camelKey: 'actorId', fallback: _eventText(data, 'id'));
        return _ActivityItem(
          icon: Icons.cancel_outlined,
          title: '取消 $actorId',
          detail: _eventText(data, 'reason'),
          state: _ActivityState.failed,
          kind: 'actor',
          actorId: actorId,
        );
    }
    return null;
  }

  void _appendActivity(_ActivityItem item) {
    final round = _ensureAssistantRound();
    round.activities.add(item);
    if (round.activities.length > 16) {
      round.activities.removeRange(0, round.activities.length - 16);
    }
    round.isStreaming = true;
    round.activityExpanded = true;
    _status = 'streaming';
  }

  _ConversationRound _ensureAssistantRound() {
    if (_rounds.isNotEmpty && !_rounds.last.isComplete) {
      return _rounds.last;
    }
    final round = _ConversationRound(
      index: _rounds.length + 1,
      userText: '',
    );
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

  void _scrollToBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_scrollCtrl.hasClients) {
        final media = MediaQuery.maybeOf(context);
        final reduceMotion = media?.disableAnimations == true ||
            media?.accessibleNavigation == true;
        if (reduceMotion) {
          _scrollCtrl.jumpTo(_scrollCtrl.position.maxScrollExtent);
        } else {
          _scrollCtrl.animateTo(
            _scrollCtrl.position.maxScrollExtent,
            duration: const Duration(milliseconds: 240),
            curve: Curves.easeOut,
          );
        }
      }
    });
  }

  Future<void> _send() async {
    final text = _inputCtrl.text.trim();
    if (text.isEmpty) return;
    await _ensureSession();
    setState(() {
      _rounds.add(_ConversationRound(
        index: _rounds.length + 1,
        userText: text,
      ));
      _status = 'streaming';
      _canSend = false;
    });
    _inputCtrl.clear();
    await widget.client.sendMessage(_sessionId!, text);
    _scrollToBottom();
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

  Future<void> _loadProject() async {
    final id = _sessionId;
    if (id == null) return;
    final ov = await widget.client.projectOverview(id);
    if (!mounted) return;
    setState(() {
      _projectStatus = ov.status;
      _nextActions
        ..clear()
        ..addAll(ov.nextActions);
    });
  }

  Future<void> _promoteSession() async {
    await _ensureSession();
    final last = _rounds
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
      setState(() =>
          _projectStatus = '${p.projectUri} · ${p.promotedCount} promoted');
    } catch (e) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Promote failed: $e')));
    }
  }

  List<String> _extractResources(String text) {
    return RegExp(r'\b(?:artifact|workspace|linked|project)://[^\s),\]]+')
        .allMatches(text)
        .map((m) => m.group(0)!.replaceAll(RegExp(r'[.。]+$'), ''))
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
    return uri.trim().replaceAll(RegExp(r'[.。]+$'), '');
  }

  Future<void> _openResource(String uri) async {
    await _ensureSession();
    try {
      final preview = await widget.client.previewResource(_sessionId!, uri);
      if (!mounted) return;
      await showModalBottomSheet<void>(
        context: context,
        showDragHandle: true,
        isScrollControlled: true,
        backgroundColor: _tok.surface,
        builder: (_) =>
            _ResourceSheet(preview: preview, tok: _tok, copy: _copy),
      );
    } catch (err) {
      if (!mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Could not open $uri: $err')));
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
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Mode change failed: $err')));
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
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Mode lock failed: $err')));
    }
  }

  Future<void> _loadHistoricalSession(String sessionId) async {
    if (mounted) setState(() => _status = 'connecting');
    try {
      final loaded = await widget.client.loadSession(sessionId);
      await _attachSession(
        loaded.session,
        messages: loaded.messages,
        pendingEvents: loaded.pendingEvents,
      );
    } catch (err) {
      if (!mounted) return;
      setState(() => _status = 'offline');
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('History load failed: $err')));
    }
  }

  Future<void> _startNewSession() async {
    if (mounted) setState(() => _status = 'connecting');
    try {
      final session = await widget.client.createSession();
      await _attachSession(session);
    } catch (err) {
      if (!mounted) return;
      setState(() => _status = 'offline');
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('New session failed: $err')));
    }
  }

  // ── Bottom sheets ──────────────────────────────────────────────────────────

  void _showHistorySheet() {
    final tok = _tok;
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      isScrollControlled: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder: (sheetContext) => ConstrainedBox(
        constraints: BoxConstraints(
          maxHeight: MediaQuery.of(sheetContext).size.height * 0.86,
        ),
        child: _SessionHistorySheet(
          tok: tok,
          copy: _copy,
          currentSessionId: _sessionId,
          loadSessions: () => widget.client.listSessions(),
          onSelect: (id) {
            Navigator.pop(sheetContext);
            unawaited(_loadHistoricalSession(id));
          },
          onNewSession: () {
            Navigator.pop(sheetContext);
            unawaited(_startNewSession());
          },
        ),
      ),
    );
  }

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
      builder: (sheetContext) => ConstrainedBox(
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
      builder: (_) => _ApprovalSheet(
        approval: a,
        tok: _tok,
        copy: _copy,
        accent: _accent,
        onOption: (option) {
          final isReject = option.kind.startsWith('reject') ||
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
      builder: (sheetContext) => ConstrainedBox(
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

  void _showOverflowSheet() {
    final tok = _tok;
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(20)),
      ),
      builder: (_) => _OverflowSheet(
        tok: tok,
        copy: _copy,
        projectStatus: _projectStatus,
        nextActions: _nextActions,
        isDark: _isDark,
        onRefresh: () {
          Navigator.pop(context);
          _loadProject();
        },
        onPromote: () {
          Navigator.pop(context);
          _promoteSession();
        },
        onThemeToggle: () {
          Navigator.pop(context);
          setState(() => _isDark = !_isDark);
        },
        onModeSettings: () {
          Navigator.pop(context);
          Timer(const Duration(milliseconds: 320), () {
            if (mounted) _showModeSheet();
          });
        },
      ),
    );
  }

  // ── Build ──────────────────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    final tok = _tok;
    final mode = _mode;
    final accent = _accent;

    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: _isDark ? SystemUiOverlayStyle.light : SystemUiOverlayStyle.dark,
      child: Scaffold(
        backgroundColor: tok.bg,
        body: SafeArea(
          bottom: false,
          child: Column(
            children: [
              _buildHeader(tok, mode, accent),
              Expanded(child: _buildThread(tok, accent)),
              _buildComposer(tok, accent),
              SafeArea(
                top: false,
                child: SizedBox(
                  height: 20,
                  child: Center(
                    child: Container(
                      width: 128,
                      height: 5,
                      decoration: BoxDecoration(
                        color: tok.text.withOpacity(0.3),
                        borderRadius: BorderRadius.circular(999),
                      ),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildHeader(_Tok tok, _Mode mode, Color accent) {
    final copy = _copy;
    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 430;
        final title = compact ? 'Miku' : 'TempestMiku';
        return Container(
          decoration: BoxDecoration(
            color: tok.bg,
            border: Border(
              bottom:
                  BorderSide(color: tok.border.withOpacity(0.6), width: 0.5),
            ),
          ),
          padding: EdgeInsets.fromLTRB(compact ? 14 : 16, 8, 14, 10),
          child: Row(
            children: [
              Semantics(
                label: 'TempestMiku',
                image: true,
                child: Container(
                  width: 34,
                  height: 34,
                  decoration: BoxDecoration(
                    color: accent,
                    borderRadius: BorderRadius.circular(10),
                    boxShadow: [
                      BoxShadow(
                        color: accent.withOpacity(0.3),
                        blurRadius: 8,
                        offset: const Offset(0, 3),
                      ),
                    ],
                  ),
                  child:
                      Icon(Icons.smart_toy, color: _textOn(accent), size: 19),
                ),
              ),
              SizedBox(width: compact ? 8 : 10),
              Expanded(
                child: Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: compact ? 15 : 15.5,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              _ModeDropMenuButton(
                tok: tok,
                copy: copy,
                mode: mode,
                accent: accent,
                locked: _modeLocked,
                compact: compact,
                onTap: _showModeSheet,
              ),
              SizedBox(width: compact ? 6 : 8),
              _ConnectionBadge(
                status: _status,
                tok: tok,
                copy: copy,
                compact: compact,
              ),
              SizedBox(width: compact ? 6 : 8),
              _LanguageToggle(
                tok: tok,
                copy: copy,
                onTap: () {
                  setState(() {
                    _language = _language == _UiLanguage.en
                        ? _UiLanguage.zh
                        : _UiLanguage.en;
                  });
                },
              ),
              SizedBox(width: compact ? 6 : 8),
              _TokIconBtn(
                tok: tok,
                icon: Icons.history,
                tooltip: copy.sessions,
                semanticLabel: copy.openSessions,
                onTap: _showHistorySheet,
              ),
              SizedBox(width: compact ? 6 : 8),
              _TokIconBtn(
                tok: tok,
                icon: Icons.more_horiz,
                tooltip: copy.more,
                semanticLabel: copy.openMore,
                onTap: _showOverflowSheet,
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _buildThread(_Tok tok, Color accent) {
    final copy = _copy;
    final items = <Widget>[];

    // Connection system line
    items.add(
      _SystemLine(
        tok: tok,
        text: _sessionId != null
            ? copy.systemConnected(_lastEventId)
            : copy.systemOffline,
      ),
    );

    if (_rounds.isEmpty && _memoryProposals.isEmpty && _approvals.isEmpty) {
      items.add(
        _EmptyState(tok: tok, accent: accent, status: _status, copy: copy),
      );
      items.add(const SizedBox(height: 14));
    }

    for (final round in _rounds) {
      items.add(_RoundLabel(tok: tok, copy: copy, index: round.index));
      items.add(const SizedBox(height: 8));
      if (round.userText.isNotEmpty) {
        items.add(
          _UserBubble(tok: tok, text: round.userText, accent: tok.accentSoft),
        );
        items.add(const SizedBox(height: 10));
      }

      if (round.activities.isNotEmpty) {
        items.add(
          _AgentStatusBar(
            tok: tok,
            copy: copy,
            accent: accent,
            anim: _dotAnim,
            agents: _agentStatuses(round.activities),
            activities: round.activities,
            expanded: round.activityExpanded,
            onTap: () => _showActivitySheet(round),
            onOpenResource: _openResource,
          ),
        );
        items.add(const SizedBox(height: 10));
      }

      final assistantText = round.assistantText;
      if (assistantText.isNotEmpty) {
        final resources = _extractResources(assistantText);
        items.add(_MikuBubble(
          tok: tok,
          copy: copy,
          text: assistantText,
          accent: accent,
          resources: resources,
          onOpenResource: _openResource,
          isStreaming: round.assistantFinalText.isEmpty && round.isStreaming,
        ));
      } else if (round.isStreaming && round.activities.isEmpty) {
        items.add(_TypingIndicator(tok: tok, accent: accent, anim: _dotAnim));
      }
      items.add(const SizedBox(height: 14));
    }

    // Pending memory proposals
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

    // Pending approvals
    for (final a in _approvals.where((a) => !_isRenderedAsMemoryProposal(a))) {
      items.add(
        _ApprovalCard(
          tok: tok,
          copy: copy,
          approval: a,
          accent: accent,
          onTap: () => _showApprovalSheet(a),
        ),
      );
      items.add(const SizedBox(height: 10));
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        return Center(
          child: SizedBox(
            width: math.min(constraints.maxWidth, 720),
            height: constraints.maxHeight,
            child: ListView(
              controller: _scrollCtrl,
              padding: const EdgeInsets.fromLTRB(14, 10, 14, 16),
              children: items,
            ),
          ),
        );
      },
    );
  }

  Widget _buildComposer(_Tok tok, Color accent) {
    final copy = _copy;
    return LayoutBuilder(
      builder: (context, constraints) {
        return Container(
          padding: const EdgeInsets.fromLTRB(14, 8, 14, 10),
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topCenter,
              end: Alignment.bottomCenter,
              colors: [tok.bg.withOpacity(0), tok.bg],
              stops: const [0, 0.22],
            ),
          ),
          child: Center(
            child: SizedBox(
              width: math.min(constraints.maxWidth, 720),
              child: Container(
                decoration: BoxDecoration(
                  color: tok.raised,
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(14),
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    Expanded(
                      child: Semantics(
                        label: copy.messageField,
                        textField: true,
                        child: TextField(
                          controller: _inputCtrl,
                          style: TextStyle(color: tok.text, fontSize: 13.5),
                          minLines: 1,
                          maxLines: 6,
                          keyboardType: TextInputType.multiline,
                          textInputAction: TextInputAction.send,
                          decoration: InputDecoration(
                            hintText: copy.messageHint,
                            hintStyle:
                                TextStyle(color: tok.muted, fontSize: 13.5),
                            border: InputBorder.none,
                            contentPadding:
                                const EdgeInsets.fromLTRB(14, 10, 8, 10),
                          ),
                          onChanged: (value) {
                            final canSend = value.trim().isNotEmpty;
                            if (canSend != _canSend) {
                              setState(() => _canSend = canSend);
                            }
                          },
                          onSubmitted: (_) => _send(),
                        ),
                      ),
                    ),
                    Padding(
                      padding: const EdgeInsets.all(5),
                      child: Tooltip(
                        message: _canSend ? copy.send : copy.typeMessage,
                        child: Semantics(
                          button: true,
                          enabled: _canSend,
                          label: copy.sendMessage,
                          child: AnimatedContainer(
                            duration: const Duration(milliseconds: 140),
                            width: 40,
                            height: 40,
                            decoration: BoxDecoration(
                              color: _canSend
                                  ? accent
                                  : tok.border.withOpacity(0.55),
                              borderRadius: BorderRadius.circular(11),
                            ),
                            child: IconButton(
                              padding: EdgeInsets.zero,
                              constraints: const BoxConstraints(),
                              onPressed: _canSend ? _send : null,
                              icon: Icon(
                                Icons.send,
                                color: _canSend
                                    ? _textOn(accent)
                                    : tok.muted.withOpacity(0.72),
                                size: 17,
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

String _eventText(
  Map<String, Object?> data,
  String key, {
  String? camelKey,
  String fallback = '',
}) {
  final value = data[key] ?? (camelKey == null ? null : data[camelKey]);
  final text = value?.toString() ?? '';
  return text.isEmpty ? fallback : text;
}

List<_AgentStatus> _agentStatuses(List<_ActivityItem> activities) {
  final ordered = <String>[];
  final roles = <String, String>{};
  final states = <String, _ActivityState>{};
  final details = <String, String>{};

  for (final item in activities) {
    final id = item.actorId;
    if (item.kind != 'actor' || id == null || id.isEmpty) continue;
    if (!ordered.contains(id)) ordered.add(id);
    final role = item.role;
    if (role != null && role.isNotEmpty) {
      roles[id] = role;
    }
    states[id] = item.state;
    if (item.detail.trim().isNotEmpty) {
      details[id] = item.detail.trim();
    } else {
      details[id] = item.title;
    }
  }

  return ordered
      .map(
        (id) => _AgentStatus(
          id: id,
          role: roles[id] ?? _roleFromActorId(id),
          state: states[id] ?? _ActivityState.info,
          detail: details[id] ?? '',
        ),
      )
      .toList();
}

String _roleFromActorId(String id) {
  final match = RegExp(r'^([A-Za-z_ -]+)').firstMatch(id);
  final role = match?.group(1)?.trim() ?? '';
  return role.isEmpty ? 'agent' : role.toLowerCase();
}
