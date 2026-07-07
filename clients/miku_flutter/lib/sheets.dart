part of 'main.dart';

// ─── Bottom sheet widgets ──────────────────────────────────────────────────────

class _ModeSheet extends StatelessWidget {
  const _ModeSheet({
    required this.modes,
    required this.currentId,
    required this.locked,
    required this.tok,
    required this.copy,
    required this.onPick,
    required this.onLockToggle,
  });

  final List<_Mode> modes;
  final String currentId;
  final bool locked;
  final _Tok tok;
  final _UiCopy copy;
  final void Function(String) onPick;
  final VoidCallback onLockToggle;

  @override
  Widget build(BuildContext context) {
    final current = _findMode(currentId, modes);
    final currentAccent = _modeAccent(current.temp, tok);
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(15, 9, 15, 18),
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
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      copy.modeSheetTitle,
                      style: TextStyle(
                        color: tok.text,
                        fontSize: 17,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      copy.modeSheetHelper,
                      style: TextStyle(
                        color: tok.muted,
                        fontSize: 12,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ],
                ),
              ),
              _TokIconBtn(
                tok: tok,
                icon: Icons.close,
                tooltip: copy.close,
                semanticLabel: copy.closeModeSheet,
                onTap: () => Navigator.pop(context),
              ),
            ],
          ),
          const SizedBox(height: 13),
          Semantics(
            button: true,
            label: locked ? copy.unlockMode(current) : copy.lockMode(current),
            child: Material(
              color: Colors.transparent,
              child: InkWell(
                onTap: onLockToggle,
                borderRadius: BorderRadius.circular(13),
                focusColor: tok.focus.withOpacity(0.18),
                child: Container(
                  padding: const EdgeInsets.fromLTRB(13, 12, 13, 12),
                  decoration: BoxDecoration(
                    color: locked ? currentAccent.withOpacity(0.1) : tok.bg,
                    border: Border.all(
                      color:
                          locked ? currentAccent.withOpacity(0.62) : tok.border,
                    ),
                    borderRadius: BorderRadius.circular(13),
                  ),
                  child: Row(
                    children: [
                      Icon(
                        locked ? Icons.lock : Icons.lock_open,
                        color: locked ? currentAccent : tok.muted,
                        size: 19,
                      ),
                      const SizedBox(width: 11),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              locked
                                  ? copy.unlockMode(current)
                                  : copy.lockMode(current),
                              style: TextStyle(
                                color: tok.text,
                                fontSize: 13.5,
                                fontWeight: FontWeight.w800,
                              ),
                            ),
                            const SizedBox(height: 1),
                            Text(
                              locked
                                  ? copy.unlockModeHelper
                                  : copy.lockModeHelper,
                              style: TextStyle(
                                color: tok.muted,
                                fontSize: 11,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ],
                        ),
                      ),
                      _Toggle(on: locked, accent: currentAccent, tok: tok),
                    ],
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 12),
          ...modes.map((m) {
            final isActive = m.id == currentId;
            final mAccent = _modeAccent(m.temp, tok);
            return Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Semantics(
                button: true,
                selected: isActive,
                label: copy.selectMode(m),
                child: Material(
                  color: Colors.transparent,
                  child: InkWell(
                    onTap: () => onPick(m.id),
                    borderRadius: BorderRadius.circular(13),
                    focusColor: tok.focus.withOpacity(0.18),
                    child: Container(
                      padding: const EdgeInsets.fromLTRB(12, 11, 12, 11),
                      decoration: BoxDecoration(
                        color: isActive ? mAccent.withOpacity(0.08) : tok.bg,
                        border: Border.all(
                          color: isActive ? mAccent : tok.border,
                        ),
                        borderRadius: BorderRadius.circular(13),
                      ),
                      child: Row(
                        children: [
                          Container(
                            width: 38,
                            height: 38,
                            decoration: BoxDecoration(
                              color: mAccent,
                              borderRadius: BorderRadius.circular(10),
                            ),
                            child: Icon(
                              m.icon,
                              color: _textOn(mAccent),
                              size: 20,
                            ),
                          ),
                          const SizedBox(width: 12),
                          Expanded(
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Text(
                                  m.label,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                    color: tok.text,
                                    fontSize: 14,
                                    fontWeight: FontWeight.w800,
                                  ),
                                ),
                                const SizedBox(height: 2),
                                Text(
                                  m.tip,
                                  style: TextStyle(
                                    color: tok.muted,
                                    fontSize: 11.5,
                                    fontWeight: FontWeight.w500,
                                    height: 1.4,
                                  ),
                                ),
                              ],
                            ),
                          ),
                          if (isActive)
                            Container(
                              width: 22,
                              height: 22,
                              decoration: BoxDecoration(
                                shape: BoxShape.circle,
                                color: mAccent,
                              ),
                              child: Icon(
                                Icons.check,
                                color: _textOn(mAccent),
                                size: 14,
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            );
          }),
        ],
      ),
    );
  }
}

class _AgentActivitySheet extends StatelessWidget {
  const _AgentActivitySheet({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.roundIndex,
    required this.agents,
    required this.activities,
    required this.onOpenResource,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final int roundIndex;
  final List<_AgentStatus> agents;
  final List<_ActivityItem> activities;
  final void Function(String) onOpenResource;

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.fromLTRB(15, 9, 15, 18),
      children: [
        Center(
          child: Container(
            width: 38,
            height: 5,
            decoration: BoxDecoration(
              color: tok.border,
              borderRadius: BorderRadius.circular(999),
            ),
          ),
        ),
        const SizedBox(height: 14),
        Row(
          children: [
            Container(
              width: 38,
              height: 38,
              decoration: BoxDecoration(
                color: accent,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Icon(
                Icons.account_tree_outlined,
                color: _textOn(accent),
                size: 20,
              ),
            ),
            const SizedBox(width: 11),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    copy.agentsSheetTitle(roundIndex),
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 17,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    copy.agentCount(agents.length, activities.length),
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ],
              ),
            ),
            _TokIconBtn(
              tok: tok,
              icon: Icons.close,
              tooltip: copy.close,
              semanticLabel: copy.closeActivitySheet,
              onTap: () => Navigator.pop(context),
            ),
          ],
        ),
        if (agents.isNotEmpty) ...[
          const SizedBox(height: 14),
          Text(
            copy.status,
            style: TextStyle(
              color: tok.text,
              fontSize: 12,
              fontWeight: FontWeight.w900,
            ),
          ),
          const SizedBox(height: 8),
          for (final agent in agents) ...[
            _AgentSheetRow(tok: tok, copy: copy, accent: accent, agent: agent),
            if (agent != agents.last) const SizedBox(height: 7),
          ],
        ],
        const SizedBox(height: 14),
        Text(
          copy.promptActivity,
          style: TextStyle(
            color: tok.text,
            fontSize: 12,
            fontWeight: FontWeight.w900,
          ),
        ),
        const SizedBox(height: 8),
        Container(
          padding: const EdgeInsets.fromLTRB(11, 10, 11, 11),
          decoration: BoxDecoration(
            color: tok.bg,
            border: Border.all(color: tok.border),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Column(
            children: [
              for (var i = 0; i < activities.length; i++) ...[
                _ActivityRow(
                  tok: tok,
                  copy: copy,
                  accent: accent,
                  item: activities[i],
                  onOpenResource: onOpenResource,
                ),
                if (i != activities.length - 1) ...[
                  const SizedBox(height: 9),
                  Container(height: 0.5, color: tok.border.withOpacity(0.7)),
                  const SizedBox(height: 9),
                ],
              ],
            ],
          ),
        ),
      ],
    );
  }
}

class _AgentSheetRow extends StatelessWidget {
  const _AgentSheetRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.agent,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final _AgentStatus agent;

  @override
  Widget build(BuildContext context) {
    final color = agent.isRunning ? accent : tok.muted;
    return Container(
      padding: const EdgeInsets.fromLTRB(10, 9, 10, 9),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            agent.isRunning
                ? Icons.radio_button_checked
                : Icons.stop_circle_outlined,
            color: color,
            size: 16,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        '${agent.role} agent · ${agent.id}',
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.5,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Text(
                      copy.stateLabel(agent.state),
                      style: TextStyle(
                        color: color,
                        fontSize: 10.5,
                        fontWeight: FontWeight.w900,
                      ),
                    ),
                  ],
                ),
                if (agent.detail.isNotEmpty) ...[
                  const SizedBox(height: 4),
                  Text(
                    agent.detail,
                    maxLines: 4,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11.5,
                      fontWeight: FontWeight.w600,
                      height: 1.36,
                    ),
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

class _SessionHistorySheet extends StatefulWidget {
  const _SessionHistorySheet({
    required this.tok,
    required this.copy,
    required this.currentSessionId,
    required this.loadSessions,
    required this.onSelect,
    required this.onNewSession,
  });

  final _Tok tok;
  final _UiCopy copy;
  final String? currentSessionId;
  final Future<List<SessionSummary>> Function() loadSessions;
  final void Function(String sessionId) onSelect;
  final VoidCallback onNewSession;

  @override
  State<_SessionHistorySheet> createState() => _SessionHistorySheetState();
}

class _SessionHistorySheetState extends State<_SessionHistorySheet> {
  late Future<List<SessionSummary>> _future;

  @override
  void initState() {
    super.initState();
    _future = widget.loadSessions();
  }

  void _refresh() {
    setState(() => _future = widget.loadSessions());
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final copy = widget.copy;
    return Padding(
      padding: const EdgeInsets.fromLTRB(15, 9, 15, 18),
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
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      copy.sessions,
                      style: TextStyle(
                        color: tok.text,
                        fontSize: 17,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      copy.historyHelper,
                      style: TextStyle(
                        color: tok.muted,
                        fontSize: 12,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ],
                ),
              ),
              _TokIconBtn(
                tok: tok,
                icon: Icons.refresh,
                tooltip: copy.refresh,
                semanticLabel: copy.refreshSessions,
                onTap: _refresh,
              ),
              const SizedBox(width: 8),
              _TokIconBtn(
                tok: tok,
                icon: Icons.add,
                tooltip: copy.newSession,
                semanticLabel: copy.createNewSession,
                onTap: widget.onNewSession,
              ),
              const SizedBox(width: 8),
              _TokIconBtn(
                tok: tok,
                icon: Icons.close,
                tooltip: copy.close,
                semanticLabel: copy.closeSessions,
                onTap: () => Navigator.pop(context),
              ),
            ],
          ),
          const SizedBox(height: 13),
          Flexible(
            child: FutureBuilder<List<SessionSummary>>(
              future: _future,
              builder: (context, snapshot) {
                if (snapshot.connectionState == ConnectionState.waiting) {
                  return _HistoryStateLine(
                    tok: tok,
                    icon: Icons.hourglass_top,
                    text: copy.loadingSessions,
                  );
                }
                if (snapshot.hasError) {
                  return _HistoryStateLine(
                    tok: tok,
                    icon: Icons.error_outline,
                    text: copy.historyLoadFailed(snapshot.error!),
                  );
                }
                final sessions = snapshot.data ?? const [];
                if (sessions.isEmpty) {
                  return _HistoryStateLine(
                    tok: tok,
                    icon: Icons.chat_bubble_outline,
                    text: copy.noSessions,
                  );
                }
                return ListView.separated(
                  shrinkWrap: true,
                  itemCount: sessions.length,
                  separatorBuilder: (_, __) => const SizedBox(height: 8),
                  itemBuilder: (context, index) {
                    final session = sessions[index];
                    return _HistorySessionRow(
                      tok: tok,
                      copy: copy,
                      session: session,
                      selected: session.id == widget.currentSessionId,
                      onTap: () => widget.onSelect(session.id),
                    );
                  },
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

class _HistorySessionRow extends StatelessWidget {
  const _HistorySessionRow({
    required this.tok,
    required this.copy,
    required this.session,
    required this.selected,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final SessionSummary session;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final accent = selected ? tok.accentSoft : tok.cool;
    return Semantics(
      button: true,
      selected: selected,
      label: copy.openSession(session.title),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(13),
          focusColor: tok.focus.withOpacity(0.18),
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 11, 12, 11),
            decoration: BoxDecoration(
              color: selected ? accent.withOpacity(0.11) : tok.bg,
              border: Border.all(color: selected ? accent : tok.border),
              borderRadius: BorderRadius.circular(13),
            ),
            child: Row(
              children: [
                Container(
                  width: 38,
                  height: 38,
                  decoration: BoxDecoration(
                    color: accent,
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Icon(
                    selected ? Icons.radio_button_checked : Icons.history,
                    color: _textOn(accent),
                    size: 19,
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        session.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 13.5,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        session.preview.isEmpty
                            ? session.label
                            : session.preview,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w500,
                          height: 1.35,
                        ),
                      ),
                      const SizedBox(height: 7),
                      Wrap(
                        spacing: 6,
                        runSpacing: 6,
                        children: [
                          _HistoryChip(
                            tok: tok,
                            icon: Icons.schedule,
                            text: _formatUpdatedAt(session.updatedAt),
                          ),
                          _HistoryChip(
                            tok: tok,
                            icon: Icons.mode_comment_outlined,
                            text: copy.messages(session.messageCount),
                          ),
                          _HistoryChip(
                            tok: tok,
                            icon: Icons.tune,
                            text: session.label,
                          ),
                        ],
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

  String _formatUpdatedAt(String value) {
    final parsed = DateTime.tryParse(value);
    if (parsed == null) return copy.recent;
    final local = parsed.toLocal();
    final month = local.month.toString().padLeft(2, '0');
    final day = local.day.toString().padLeft(2, '0');
    final hour = local.hour.toString().padLeft(2, '0');
    final minute = local.minute.toString().padLeft(2, '0');
    return '$month/$day $hour:$minute';
  }
}

class _HistoryChip extends StatelessWidget {
  const _HistoryChip({
    required this.tok,
    required this.icon,
    required this.text,
  });

  final _Tok tok;
  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 4),
      decoration: BoxDecoration(
        color: tok.surface.withOpacity(0.68),
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, color: tok.muted, size: 12),
          const SizedBox(width: 4),
          Text(
            text,
            style: TextStyle(
              color: tok.muted,
              fontSize: 10.5,
              fontWeight: FontWeight.w700,
            ),
          ),
        ],
      ),
    );
  }
}

class _HistoryStateLine extends StatelessWidget {
  const _HistoryStateLine({
    required this.tok,
    required this.icon,
    required this.text,
  });

  final _Tok tok;
  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(14, 24, 14, 24),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(13),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, color: tok.muted, size: 22),
          const SizedBox(height: 8),
          Text(
            text,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: tok.muted,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

class _Toggle extends StatelessWidget {
  const _Toggle({required this.on, required this.accent, required this.tok});

  final bool on;
  final Color accent;
  final _Tok tok;

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 160),
      width: 46,
      height: 28,
      decoration: BoxDecoration(
        color: on ? accent : tok.border,
        borderRadius: BorderRadius.circular(999),
      ),
      child: AnimatedAlign(
        duration: const Duration(milliseconds: 160),
        alignment: on ? Alignment.centerRight : Alignment.centerLeft,
        child: Padding(
          padding: const EdgeInsets.all(3),
          child: Container(
            width: 22,
            height: 22,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: Colors.white,
              boxShadow: [
                BoxShadow(
                  color: Colors.black.withOpacity(0.15),
                  blurRadius: 3,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

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
        widget.onDeny();
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
                      children: widget.approval.scope.entries.map((e) {
                        return Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 9, vertical: 3),
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
                backgroundColor: tok.border.withOpacity(0.6),
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
                          focusColor: tok.focus.withOpacity(0.18),
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
                          focusColor: tok.focus.withOpacity(0.18),
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
                children: widget.approval.options.map((option) {
                  final isReject = option.kind.startsWith('reject') ||
                      option.kind.startsWith('deny');
                  final buttonColor = isReject ? tok.bg : approveColor;
                  final textColor = isReject ? tok.text : _textOn(approveColor);
                  final rawLabel =
                      option.name.isEmpty ? option.optionId : option.name;
                  final label = isReject
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
                          focusColor: tok.focus.withOpacity(0.18),
                          child: Container(
                            width: double.infinity,
                            height: 46,
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

class _OverflowSheet extends StatelessWidget {
  const _OverflowSheet({
    required this.tok,
    required this.copy,
    required this.projectStatus,
    required this.nextActions,
    required this.isDark,
    required this.onRefresh,
    required this.onPromote,
    required this.onThemeToggle,
    required this.onModeSettings,
  });

  final _Tok tok;
  final _UiCopy copy;
  final String projectStatus;
  final List<String> nextActions;
  final bool isDark;
  final VoidCallback onRefresh, onPromote, onThemeToggle, onModeSettings;

  @override
  Widget build(BuildContext context) {
    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(16, 9, 16, 18),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Center(
            child: Container(
              width: 38,
              height: 5,
              decoration: BoxDecoration(
                color: tok.border,
                borderRadius: BorderRadius.circular(999),
              ),
            ),
          ),
          const SizedBox(height: 14),
          if (projectStatus.isNotEmpty) ...[
            Text(
              copy.projectStatus,
              style: TextStyle(
                color: tok.text,
                fontSize: 13,
                fontWeight: FontWeight.w700,
              ),
            ),
            const SizedBox(height: 6),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: tok.surface,
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(12),
              ),
              child: Text(
                projectStatus,
                style: TextStyle(
                    color: tok.text, fontSize: 12, fontWeight: FontWeight.w500),
              ),
            ),
            if (nextActions.isNotEmpty) ...[
              const SizedBox(height: 8),
              ...nextActions.map(
                (a) => Padding(
                  padding: const EdgeInsets.only(bottom: 4),
                  child: Row(
                    children: [
                      Icon(Icons.chevron_right, size: 16, color: tok.muted),
                      const SizedBox(width: 4),
                      Expanded(
                        child: Text(
                          a,
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 12.5,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ],
            const SizedBox(height: 12),
          ],
          _ActionRow(
            tok: tok,
            icon: isDark ? Icons.wb_sunny_outlined : Icons.nightlight_outlined,
            label: isDark ? copy.lightMode : copy.darkMode,
            onTap: onThemeToggle,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.refresh,
            label: copy.refreshProject,
            onTap: onRefresh,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.upload_file,
            label: copy.promoteSession,
            onTap: onPromote,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.tune,
            label: copy.modeSettings,
            onTap: onModeSettings,
          ),
        ],
      ),
    );
  }
}

class _ActionRow extends StatelessWidget {
  const _ActionRow({
    required this.tok,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final _Tok tok;
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: label,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withOpacity(0.18),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(12),
            ),
            child: Row(
              children: [
                Icon(icon, color: tok.muted, size: 18),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                    ),
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

class _ResourceSheet extends StatelessWidget {
  const _ResourceSheet({
    required this.preview,
    required this.tok,
    required this.copy,
  });

  final ResourcePreview preview;
  final _Tok tok;
  final _UiCopy copy;

  @override
  Widget build(BuildContext context) {
    final title =
        (preview.title?.isNotEmpty == true) ? preview.title! : preview.uri;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 16,
                  fontWeight: FontWeight.w700,
                ),
              ),
              const SizedBox(height: 5),
              Text(
                '${preview.kind} / ${preview.mime} / ${preview.sizeBytes} bytes',
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 12,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(height: 10),
              SelectableText(
                preview.uri,
                style: TextStyle(color: tok.muted, fontSize: 12),
              ),
              const SizedBox(height: 12),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: tok.raised,
                  border: Border.all(color: tok.border),
                  borderRadius: BorderRadius.circular(8),
                ),
                child: SelectableText(
                  preview.preview.isEmpty ? copy.emptyPreview : preview.preview,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12,
                    fontFamily: 'monospace',
                  ),
                ),
              ),
              if (preview.hasMore) ...[
                const SizedBox(height: 8),
                Text(
                  copy.previewTruncated,
                  style: TextStyle(color: tok.muted, fontSize: 12),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
