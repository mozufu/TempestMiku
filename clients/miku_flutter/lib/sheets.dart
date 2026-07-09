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
    required this.onDrive,
    required this.onThemeToggle,
    required this.onModeSettings,
    this.onServerTarget,
  });

  final _Tok tok;
  final _UiCopy copy;
  final String projectStatus;
  final List<String> nextActions;
  final bool isDark;
  final VoidCallback onRefresh,
      onPromote,
      onDrive,
      onThemeToggle,
      onModeSettings;
  final VoidCallback? onServerTarget;

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
            icon: Icons.folder_outlined,
            label: copy.driveFeed,
            onTap: onDrive,
          ),
          const SizedBox(height: 8),
          _ActionRow(
            tok: tok,
            icon: Icons.tune,
            label: copy.modeSettings,
            onTap: onModeSettings,
          ),
          if (onServerTarget != null) ...[
            const SizedBox(height: 8),
            _ActionRow(
              tok: tok,
              icon: Icons.dns_outlined,
              label: copy.serverTarget,
              onTap: onServerTarget!,
            ),
          ],
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

class _DriveFeedSheet extends StatefulWidget {
  const _DriveFeedSheet({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.initialFeed,
    required this.initialError,
    required this.initialLoading,
    required this.approvals,
    required this.loadFeed,
    required this.onOpenResource,
    required this.onOpenApproval,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveFeed? initialFeed;
  final String initialError;
  final bool initialLoading;
  final List<ApprovalPrompt> approvals;
  final Future<DriveFeed> Function() loadFeed;
  final void Function(String uri) onOpenResource;
  final void Function(ApprovalPrompt approval) onOpenApproval;

  @override
  State<_DriveFeedSheet> createState() => _DriveFeedSheetState();
}

class _DriveFeedSheetState extends State<_DriveFeedSheet> {
  DriveFeed? _feed;
  Object? _error;
  bool _loading = false;

  @override
  void initState() {
    super.initState();
    _feed = widget.initialFeed;
    _error = widget.initialError.isEmpty ? null : widget.initialError;
    _loading = widget.initialLoading || widget.initialFeed == null;
    unawaited(_refresh(silent: widget.initialFeed != null));
  }

  Future<void> _refresh({bool silent = false}) async {
    if (!silent && mounted) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final feed = await widget.loadFeed();
      if (!mounted) return;
      setState(() {
        _feed = feed;
        _loading = false;
        _error = null;
      });
    } catch (err) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = err;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final copy = widget.copy;
    final feed = _feed;
    final hasPendingDriveApprovals = widget.approvals.isNotEmpty ||
        (feed?.pendingApprovals.isNotEmpty ?? false);
    final showEmptyFeed =
        feed == null || (feed.isEmpty && !hasPendingDriveApprovals);
    final displayFeed = feed ?? DriveFeed.empty;
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
                color: widget.accent,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Icon(
                Icons.folder_outlined,
                color: _textOn(widget.accent),
                size: 20,
              ),
            ),
            const SizedBox(width: 11),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    copy.driveFeed,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 17,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    copy.driveFeedHelper,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
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
              semanticLabel: copy.refreshDrive,
              onTap: _loading ? null : () => _refresh(),
            ),
            const SizedBox(width: 8),
            _TokIconBtn(
              tok: tok,
              icon: Icons.close,
              tooltip: copy.close,
              semanticLabel: copy.closeDriveFeed,
              onTap: () => Navigator.pop(context),
            ),
          ],
        ),
        const SizedBox(height: 13),
        if (_loading && feed == null)
          _HistoryStateLine(
            tok: tok,
            icon: Icons.hourglass_top,
            text: copy.loadingDriveFeed,
          )
        else if (_error != null && feed == null)
          _HistoryStateLine(
            tok: tok,
            icon: Icons.error_outline,
            text: copy.driveFeedLoadFailed(_error!),
          )
        else if (showEmptyFeed)
          _HistoryStateLine(
            tok: tok,
            icon: Icons.folder_open_outlined,
            text: copy.noDriveFeed,
          )
        else ...[
          if (hasPendingDriveApprovals)
            _DriveSection(
              tok: tok,
              title: copy.pendingDriveApprovals,
              detail:
                  '${widget.approvals.length + displayFeed.pendingApprovals.length}',
              children: [
                for (final approval in widget.approvals)
                  _DriveApprovalRow(
                    tok: tok,
                    copy: copy,
                    approval: approval,
                    onTap: () => widget.onOpenApproval(approval),
                  ),
                for (final approval in displayFeed.pendingApprovals)
                  _DrivePendingApprovalRow(
                    tok: tok,
                    approval: approval,
                  ),
              ],
            ),
          _DriveSection(
            tok: tok,
            title: copy.recentDocuments,
            detail: copy.driveDocs(displayFeed.recent.length),
            children: displayFeed.recent.isEmpty
                ? [
                    _DriveEmptyLine(
                      tok: tok,
                      icon: Icons.folder_open_outlined,
                      text: copy.noDriveFeed,
                    ),
                  ]
                : [
                    for (final item in displayFeed.recent)
                      _DriveFeedDocRow(
                        tok: tok,
                        copy: copy,
                        accent: widget.accent,
                        item: item,
                        onOpen: () => widget.onOpenResource(item.uri),
                      ),
                  ],
          ),
          _DriveSection(
            tok: tok,
            title: copy.virtualDirs,
            detail: '${displayFeed.virtualDirs.length}',
            children: [
              Wrap(
                spacing: 7,
                runSpacing: 7,
                children: [
                  for (final dir in displayFeed.virtualDirs)
                    _DriveVirtualDirChip(
                      tok: tok,
                      dir: dir,
                      onOpen: () => widget.onOpenResource(dir.uri),
                    ),
                ],
              ),
            ],
          ),
          if (displayFeed.proposals.isNotEmpty)
            _DriveSection(
              tok: tok,
              title: copy.organizerProposals,
              detail: copy.driveProposals(displayFeed.proposals.length),
              children: [
                for (final proposal in displayFeed.proposals)
                  _DriveProposalRow(
                    tok: tok,
                    copy: copy,
                    accent: widget.accent,
                    proposal: proposal,
                    onOpen: proposal.sourceUri == null
                        ? null
                        : () => widget.onOpenResource(proposal.sourceUri!),
                  ),
              ],
            ),
          if (_loading) ...[
            const SizedBox(height: 10),
            LinearProgressIndicator(
              minHeight: 3,
              backgroundColor: tok.border.withOpacity(0.5),
              valueColor: AlwaysStoppedAnimation<Color>(widget.accent),
            ),
          ],
          if (_error != null) ...[
            const SizedBox(height: 10),
            _DriveEmptyLine(
              tok: tok,
              icon: Icons.error_outline,
              text: copy.driveFeedLoadFailed(_error!),
            ),
          ],
        ],
      ],
    );
  }
}

class _DriveSection extends StatelessWidget {
  const _DriveSection({
    required this.tok,
    required this.title,
    required this.detail,
    required this.children,
  });

  final _Tok tok;
  final String title;
  final String detail;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 13),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  title,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w900,
                  ),
                ),
              ),
              Text(
                detail,
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          ...children.expand((child) sync* {
            yield child;
            if (child != children.last) yield const SizedBox(height: 8);
          }),
        ],
      ),
    );
  }
}

class _DriveFeedDocRow extends StatelessWidget {
  const _DriveFeedDocRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.item,
    required this.onOpen,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveFeedItem item;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.openResource(item.uri),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-feed:${item.uri}'),
          onTap: onOpen,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withOpacity(0.18),
          child: Container(
            padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(12),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Container(
                  width: 34,
                  height: 34,
                  decoration: BoxDecoration(
                    color: accent.withOpacity(0.14),
                    border: Border.all(color: accent.withOpacity(0.42)),
                    borderRadius: BorderRadius.circular(9),
                  ),
                  child: Icon(
                    Icons.insert_drive_file_outlined,
                    color: accent,
                    size: 17,
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        item.displayTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 13,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        item.displayPreview,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                          height: 1.35,
                        ),
                      ),
                      const SizedBox(height: 7),
                      Wrap(
                        spacing: 6,
                        runSpacing: 6,
                        children: [
                          if (item.docKind?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.description_outlined,
                              text: item.docKind!,
                            ),
                          if (item.project?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.folder_outlined,
                              text: item.project!,
                            ),
                          if (item.tags.isNotEmpty)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.label_outline,
                              text: copy.driveTags(item.tags.length),
                            ),
                          if (item.sizeBytes != null)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.data_object,
                              text: _formatDriveBytes(item.sizeBytes!),
                            ),
                          if (item.updatedAt?.isNotEmpty == true)
                            _HistoryChip(
                              tok: tok,
                              icon: Icons.schedule,
                              text: _formatDriveUpdatedAt(
                                item.updatedAt!,
                                copy,
                              ),
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 6),
                Icon(Icons.chevron_right, color: tok.muted, size: 17),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _DriveVirtualDirChip extends StatelessWidget {
  const _DriveVirtualDirChip({
    required this.tok,
    required this.dir,
    required this.onOpen,
  });

  final _Tok tok;
  final DriveVirtualDir dir;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final label = dir.title.isEmpty ? dir.name : dir.title;
    return Semantics(
      button: true,
      label: label,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-dir:${dir.uri}'),
          onTap: onOpen,
          borderRadius: BorderRadius.circular(999),
          focusColor: tok.focus.withOpacity(0.18),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 6),
            decoration: BoxDecoration(
              color: tok.bg,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(999),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(Icons.folder_special_outlined, color: tok.muted, size: 13),
                const SizedBox(width: 6),
                ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 190),
                  child: Text(
                    label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 11.3,
                      fontWeight: FontWeight.w800,
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

class _DriveProposalRow extends StatelessWidget {
  const _DriveProposalRow({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.proposal,
    required this.onOpen,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final DriveOrganizerProposal proposal;
  final VoidCallback? onOpen;

  @override
  Widget build(BuildContext context) {
    final confidence = proposal.confidence;
    final row = Container(
      padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              color: accent.withOpacity(0.12),
              border: Border.all(color: accent.withOpacity(0.38)),
              borderRadius: BorderRadius.circular(9),
            ),
            child: Icon(Icons.rule_folder_outlined, color: accent, size: 16),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        proposal.displayTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.7,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Text(
                      proposal.status,
                      style: TextStyle(
                        color: accent,
                        fontSize: 10.5,
                        fontWeight: FontWeight.w900,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 3),
                Text(
                  proposal.displayPath,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11.3,
                    fontWeight: FontWeight.w600,
                    height: 1.35,
                  ),
                ),
                if (proposal.previewSnippet?.isNotEmpty == true) ...[
                  const SizedBox(height: 4),
                  Text(
                    proposal.previewSnippet!,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11,
                      fontWeight: FontWeight.w500,
                      height: 1.35,
                    ),
                  ),
                ],
                if (confidence != null) ...[
                  const SizedBox(height: 7),
                  _HistoryChip(
                    tok: tok,
                    icon: Icons.query_stats,
                    text: copy.driveConfidence(confidence),
                  ),
                ],
              ],
            ),
          ),
          if (onOpen != null) ...[
            const SizedBox(width: 6),
            Icon(Icons.chevron_right, color: tok.muted, size: 17),
          ],
        ],
      ),
    );
    if (onOpen == null) return row;
    return Semantics(
      button: true,
      label: proposal.sourceUri!,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onOpen,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withOpacity(0.18),
          child: row,
        ),
      ),
    );
  }
}

class _DriveApprovalRow extends StatelessWidget {
  const _DriveApprovalRow({
    required this.tok,
    required this.copy,
    required this.approval,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final ApprovalPrompt approval;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: copy.pendingApprovalSemantics(approval.action),
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('drive-approval:${approval.approvalId}'),
          onTap: onTap,
          borderRadius: BorderRadius.circular(12),
          focusColor: tok.focus.withOpacity(0.18),
          child: _DrivePendingShell(
            tok: tok,
            icon: Icons.warning_amber_rounded,
            title: approval.action,
            detail: copy.tapForDetails,
          ),
        ),
      ),
    );
  }
}

class _DrivePendingApprovalRow extends StatelessWidget {
  const _DrivePendingApprovalRow({
    required this.tok,
    required this.approval,
  });

  final _Tok tok;
  final DrivePendingApproval approval;

  @override
  Widget build(BuildContext context) {
    return _DrivePendingShell(
      tok: tok,
      icon: Icons.pending_actions,
      title: approval.action,
      detail: approval.preview ?? approval.approvalId,
    );
  }
}

class _DrivePendingShell extends StatelessWidget {
  const _DrivePendingShell({
    required this.tok,
    required this.icon,
    required this.title,
    required this.detail,
  });

  final _Tok tok;
  final IconData icon;
  final String title;
  final String detail;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
      decoration: BoxDecoration(
        color: tok.warning.withOpacity(0.1),
        border: Border.all(color: tok.warning.withOpacity(0.48)),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Icon(icon, color: tok.warning, size: 17),
          const SizedBox(width: 9),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                if (detail.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(
                    detail,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11.2,
                      fontWeight: FontWeight.w600,
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

class _DriveEmptyLine extends StatelessWidget {
  const _DriveEmptyLine({
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
      padding: const EdgeInsets.fromLTRB(12, 16, 12, 16),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(icon, color: tok.muted, size: 17),
          const SizedBox(width: 8),
          Flexible(
            child: Text(
              text,
              textAlign: TextAlign.center,
              style: TextStyle(
                color: tok.muted,
                fontSize: 12,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

String _formatDriveUpdatedAt(String value, _UiCopy copy) {
  final parsed = DateTime.tryParse(value);
  if (parsed == null) return copy.recent;
  final local = parsed.toLocal();
  final month = local.month.toString().padLeft(2, '0');
  final day = local.day.toString().padLeft(2, '0');
  final hour = local.hour.toString().padLeft(2, '0');
  final minute = local.minute.toString().padLeft(2, '0');
  return '$month/$day $hour:$minute';
}

String _formatDriveBytes(int size) {
  if (size < 1024) return '$size B';
  final kb = size / 1024;
  if (kb < 1024) return '${kb.toStringAsFixed(kb < 10 ? 1 : 0)} KB';
  final mb = kb / 1024;
  return '${mb.toStringAsFixed(mb < 10 ? 1 : 0)} MB';
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
    final body =
        preview.content.trim().isNotEmpty ? preview.content : preview.preview;
    final isPreviewOnly = preview.content.trim().isEmpty;
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
                  body.isEmpty ? copy.emptyPreview : body,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 12,
                    fontFamily: 'monospace',
                  ),
                ),
              ),
              if (isPreviewOnly && preview.hasMore) ...[
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
