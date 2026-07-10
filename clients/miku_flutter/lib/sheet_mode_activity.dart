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
                focusColor: tok.focus.withValues(alpha: 0.18),
                child: Container(
                  padding: const EdgeInsets.fromLTRB(13, 12, 13, 12),
                  decoration: BoxDecoration(
                    color:
                        locked ? currentAccent.withValues(alpha: 0.1) : tok.bg,
                    border: Border.all(
                      color: locked
                          ? currentAccent.withValues(alpha: 0.62)
                          : tok.border,
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
                    focusColor: tok.focus.withValues(alpha: 0.18),
                    child: Container(
                      padding: const EdgeInsets.fromLTRB(12, 11, 12, 11),
                      decoration: BoxDecoration(
                        color:
                            isActive ? mAccent.withValues(alpha: 0.08) : tok.bg,
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
                  Container(
                      height: 0.5, color: tok.border.withValues(alpha: 0.7)),
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
