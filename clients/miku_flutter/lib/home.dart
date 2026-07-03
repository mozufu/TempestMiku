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
  String assistantStreamedText = '';
  String assistantFinalText = '';
  bool isStreaming;

  String get assistantText => assistantFinalText.isNotEmpty
      ? assistantFinalText
      : assistantStreamedText;

  bool get isComplete => assistantFinalText.isNotEmpty && !isStreaming;
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

  Future<void>? _sessionFuture;
  StreamSubscription<MikuEvent>? _sub;
  String? _sessionId;
  String? _lastEventId;
  String _modeId = 'personal_assistant';
  String _status = 'idle';
  String _projectStatus = '';
  bool _isDark = true;
  bool _modeLocked = false;

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

  _Mode get _mode => _findMode(_modeId);
  _Tok get _tok => _isDark ? _Tok.dark : _Tok.light;
  Color get _accent => _tok.accentSoft;

  // ── Session ────────────────────────────────────────────────────────────────

  Future<void> _ensureSession() async {
    if (_sessionId != null) return;
    return _sessionFuture ??= _connectSession();
  }

  Future<void> _connectSession() async {
    if (mounted) setState(() => _status = 'connecting');
    try {
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
      _sessionId = session.id;
      _lastEventId = session.lastEventId;
      _modeId = session.mode.isEmpty ? 'personal_assistant' : session.mode;
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
        _status = 'connected';
        _loadProject();
      case 'mode':
        final newId = e.data['mode'] as String? ?? _modeId;
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
        _scrollCtrl.animateTo(
          _scrollCtrl.position.maxScrollExtent,
          duration: const Duration(milliseconds: 280),
          curve: Curves.easeOut,
        );
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
    final resources = _extractResources(last?.assistantFinalText ?? '');
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
        builder: (_) => _ResourceSheet(preview: preview, tok: _tok),
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
          modes: _kModes,
          currentId: _modeId,
          locked: _modeLocked,
          tok: tok,
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
    return Container(
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border(
          bottom: BorderSide(color: tok.border.withOpacity(0.6), width: 0.5),
        ),
      ),
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 10),
      child: Row(
        children: [
          Container(
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
            child: Icon(Icons.smart_toy, color: _textOn(accent), size: 19),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              'TempestMiku',
              style: TextStyle(
                color: tok.text,
                fontSize: 15.5,
                fontWeight: FontWeight.w800,
                letterSpacing: -0.3,
              ),
            ),
          ),
          _ModeDropMenuButton(
            tok: tok,
            mode: mode,
            accent: accent,
            locked: _modeLocked,
            onTap: _showModeSheet,
          ),
          const SizedBox(width: 8),
          _ConnectionBadge(status: _status, tok: tok),
          const SizedBox(width: 8),
          _TokIconBtn(
            tok: tok,
            icon: Icons.history,
            onTap: _showHistorySheet,
          ),
          const SizedBox(width: 8),
          _TokIconBtn(
            tok: tok,
            icon: Icons.more_horiz,
            onTap: _showOverflowSheet,
          ),
        ],
      ),
    );
  }

  Widget _buildThread(_Tok tok, Color accent) {
    final items = <Widget>[];

    // Connection system line
    items.add(
      _SystemLine(
        tok: tok,
        text: _sessionId != null
            ? '已連線到 lumo${_lastEventId != null ? ' · 從事件 #$_lastEventId 續傳' : ''}'
            : '未連線 · 傳訊息建立連線',
      ),
    );

    for (final round in _rounds) {
      items.add(_RoundLabel(tok: tok, index: round.index));
      items.add(const SizedBox(height: 8));
      if (round.userText.isNotEmpty) {
        items.add(
          _UserBubble(tok: tok, text: round.userText, accent: tok.accentSoft),
        );
        items.add(const SizedBox(height: 10));
      }

      final assistantText = round.assistantText;
      if (assistantText.isNotEmpty) {
        final resources = _extractResources(assistantText);
        items.add(_MikuBubble(
          tok: tok,
          text: assistantText,
          accent: accent,
          resources: resources,
          onOpenResource: _openResource,
          isStreaming: round.assistantFinalText.isEmpty && round.isStreaming,
        ));
      } else if (round.isStreaming) {
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
          approval: a,
          accent: accent,
          onTap: () => _showApprovalSheet(a),
        ),
      );
      items.add(const SizedBox(height: 10));
    }

    return ListView(
      controller: _scrollCtrl,
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 16),
      children: items,
    );
  }

  Widget _buildComposer(_Tok tok, Color accent) {
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
              child: TextField(
                controller: _inputCtrl,
                style: TextStyle(color: tok.text, fontSize: 13.5),
                maxLines: null,
                decoration: InputDecoration(
                  hintText: '傳訊息給 Miku…',
                  hintStyle: TextStyle(color: tok.muted, fontSize: 13.5),
                  border: InputBorder.none,
                  contentPadding: const EdgeInsets.fromLTRB(14, 10, 8, 10),
                ),
                onSubmitted: (_) => _send(),
              ),
            ),
            Padding(
              padding: const EdgeInsets.all(5),
              child: GestureDetector(
                onTap: _send,
                child: Container(
                  width: 36,
                  height: 36,
                  decoration: BoxDecoration(
                    color: accent,
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Icon(Icons.send, color: _textOn(accent), size: 17),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
