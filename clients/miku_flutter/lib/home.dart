part of 'main.dart';

// ─── Chat message model ────────────────────────────────────────────────────────

class _Msg {
  final bool isUser;
  final String text; // prefix '\x00mode:' signals a mode-change event
  final String modeId;
  const _Msg({required this.isUser, required this.text, required this.modeId});

  bool get isModeChange => text.startsWith('\x00mode:');
  String get modeChangeId => text.substring(6);
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
  final List<_Msg> _history = [];

  Future<void>? _sessionFuture;
  StreamSubscription<MikuEvent>? _sub;
  String? _sessionId;
  String? _lastEventId;
  String _modeId = 'personal_assistant';
  String _status = 'idle';
  String _streamText = '';
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
  Color get _accent => _modeAccent(_mode.temp, _tok);

  // ── Session ────────────────────────────────────────────────────────────────

  Future<void> _ensureSession() async {
    if (_sessionId != null) return;
    return _sessionFuture ??= _connectSession();
  }

  Future<void> _connectSession() async {
    if (mounted) setState(() => _status = 'connecting');
    try {
      final s = await widget.client.createOrReuseSession();
      if (!mounted) return;
      await _sub?.cancel();
      _sessionId = s.id;
      _lastEventId = s.lastEventId;
      _modeId = s.mode;
      _modeLocked = s.locked;
      _status = 'connected';
      _sub = widget.client
          .events(s.id, lastEventId: _lastEventId)
          .listen(_onEvent, onError: (_) {
        if (mounted) setState(() => _status = 'reconnecting');
      });
      await _loadProject();
      if (mounted) setState(() {});
    } catch (err) {
      _sessionFuture = null;
      if (!mounted) return;
      setState(() => _status = 'offline');
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not connect to tm-server: $err')),
      );
    }
  }

  void _onEvent(MikuEvent e) {
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
    setState(() {
      switch (e.type) {
        case 'connection':
          _status = e.data['status'] as String? ?? _status;
        case 'text':
          _streamText += e.data['delta'] as String? ?? '';
          if (_streamText.isNotEmpty) _status = 'streaming';
        case 'final':
          final text = e.data['text'] as String? ?? '';
          _history.add(_Msg(isUser: false, text: text, modeId: _modeId));
          _streamText = '';
          _status = 'connected';
          _loadProject();
        case 'mode':
          final newId = e.data['mode'] as String? ?? _modeId;
          _modeLocked =
              e.data['lockSource'] != null || e.data['lock_source'] != null;
          if (newId != _modeId) {
            _history.add(
                _Msg(isUser: false, text: '\x00mode:$newId', modeId: newId));
            _modeId = newId;
          }
        case 'approval':
          final approval = ApprovalPrompt(
            approvalId: e.data['approvalId'] as String? ?? '',
            action: e.data['action'] as String? ?? 'Approval requested',
            scope:
                (e.data['scope'] as Map?)?.cast<String, Object?>() ?? const {},
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
    });
    _scrollToBottom();
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
      _history.add(_Msg(isUser: true, text: text, modeId: _modeId));
      _status = 'streaming';
      _streamText = '';
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
    final last = _history.where((m) => !m.isUser && !m.isModeChange).lastOrNull;
    final resources = _extractResources(last?.text ?? '');
    try {
      final p = await widget.client.promoteSession(
        _sessionId!,
        summary: last?.text,
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

  // ── Bottom sheets ──────────────────────────────────────────────────────────

  void _showModeSheet() {
    final tok = _tok;
    final accent = _accent;
    showModalBottomSheet<void>(
      context: context,
      backgroundColor: tok.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      builder: (_) => _ModeSheet(
        modes: _kModes,
        currentId: _modeId,
        locked: _modeLocked,
        tok: tok,
        accent: accent,
        onPick: (id) {
          setState(() => _modeId = id);
          Navigator.pop(context);
          if (_modeLocked && _sessionId != null) {
            widget.client.lockMode(_sessionId!, id);
          }
        },
        onLockToggle: () {
          final wasLocked = _modeLocked;
          setState(() => _modeLocked = !_modeLocked);
          Navigator.pop(context);
          if (_sessionId != null) {
            if (!wasLocked) {
              widget.client.lockMode(_sessionId!, _modeId);
            } else {
              widget.client.unlockMode(_sessionId!);
            }
          }
        },
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
              _buildModeRail(tok, mode, accent),
              const SizedBox(height: 6),
              Expanded(child: _buildThread(tok, mode, accent)),
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
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
            decoration: BoxDecoration(
              color: accent.withOpacity(0.12),
              border: Border.all(color: accent.withOpacity(0.45)),
              borderRadius: BorderRadius.circular(999),
            ),
            child: Text(
              _modeLocked ? '${mode.short} · locked' : mode.short,
              style: TextStyle(
                color: accent,
                fontSize: 11,
                fontWeight: FontWeight.w800,
              ),
            ),
          ),
          const SizedBox(width: 8),
          _ConnectionBadge(status: _status, tok: tok),
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

  Widget _buildModeRail(_Tok tok, _Mode mode, Color accent) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 0),
      child: Container(
        padding: const EdgeInsets.all(4),
        decoration: BoxDecoration(
          color: tok.surface,
          border: Border.all(color: tok.border),
          borderRadius: BorderRadius.circular(14),
        ),
        child: Row(
          children: _kModes.map((m) {
            final isActive = m.id == _modeId;
            final mAccent = _modeAccent(m.temp, tok);
            return Expanded(
              child: GestureDetector(
                onTap: () {
                  setState(() => _modeId = m.id);
                  if (_modeLocked && _sessionId != null) {
                    widget.client.lockMode(_sessionId!, m.id);
                  }
                },
                onLongPress: _showModeSheet,
                child: AnimatedContainer(
                  duration: const Duration(milliseconds: 200),
                  padding: const EdgeInsets.symmetric(vertical: 7),
                  decoration: BoxDecoration(
                    color: isActive ? mAccent : Colors.transparent,
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(
                        m.icon,
                        size: 18,
                        color: isActive ? _textOn(mAccent) : tok.muted,
                      ),
                      const SizedBox(height: 3),
                      Text(
                        m.short,
                        style: TextStyle(
                          fontSize: 10,
                          fontWeight: FontWeight.w700,
                          color: isActive ? _textOn(mAccent) : tok.muted,
                          letterSpacing: -0.2,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            );
          }).toList(),
        ),
      ),
    );
  }

  Widget _buildThread(_Tok tok, _Mode mode, Color accent) {
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

    // History
    for (final msg in _history) {
      if (msg.isModeChange) {
        final m = _findMode(msg.modeChangeId);
        items.add(_ModeChangeEvent(tok: tok, modeZh: m.zh));
      } else if (msg.isUser) {
        items.add(
          _UserBubble(tok: tok, text: msg.text, accent: tok.accentSoft),
        );
      } else {
        final m = _findMode(msg.modeId);
        final ma = _modeAccent(m.temp, tok);
        final resources = _extractResources(msg.text);
        items.add(_MikuBubble(
          tok: tok,
          text: msg.text,
          mode: m,
          accent: ma,
          resources: resources,
          onOpenResource: _openResource,
        ));
      }
      items.add(const SizedBox(height: 14));
    }

    // Current stream
    if (_status == 'streaming') {
      if (_streamText.isNotEmpty) {
        final resources = _extractResources(_streamText);
        items.add(_MikuBubble(
          tok: tok,
          text: _streamText,
          mode: mode,
          accent: accent,
          resources: resources,
          onOpenResource: _openResource,
          isStreaming: true,
        ));
      } else {
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
