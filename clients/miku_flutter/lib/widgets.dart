part of 'main.dart';

// ─── Small reusable widgets ────────────────────────────────────────────────────

class _TokIconBtn extends StatelessWidget {
  const _TokIconBtn({
    required this.tok,
    required this.icon,
    required this.onTap,
  });

  final _Tok tok;
  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        width: 32,
        height: 32,
        decoration: BoxDecoration(
          border: Border.all(color: tok.border),
          borderRadius: BorderRadius.circular(9),
        ),
        child: Icon(icon, color: tok.muted, size: 16),
      ),
    );
  }
}

class _ModeDropMenuButton extends StatelessWidget {
  const _ModeDropMenuButton({
    required this.tok,
    required this.mode,
    required this.accent,
    required this.locked,
    required this.onTap,
  });

  final _Tok tok;
  final _Mode mode;
  final Color accent;
  final bool locked;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final borderColor = locked ? accent.withOpacity(0.58) : tok.border;
    final bg = locked ? accent.withOpacity(0.12) : tok.surface.withOpacity(0.6);
    final label = locked ? '${mode.short}鎖定' : mode.short;
    return Tooltip(
      message: locked ? '模式已鎖定，點擊管理' : '切換模式',
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          height: 32,
          constraints: const BoxConstraints(minWidth: 58, maxWidth: 92),
          padding: const EdgeInsets.symmetric(horizontal: 9),
          decoration: BoxDecoration(
            color: bg,
            border: Border.all(color: borderColor),
            borderRadius: BorderRadius.circular(999),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                locked ? Icons.lock : mode.icon,
                color: locked ? accent : tok.muted,
                size: 13,
              ),
              const SizedBox(width: 5),
              Flexible(
                child: Text(
                  label,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: locked ? accent : tok.muted,
                    fontSize: 11,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              const SizedBox(width: 2),
              Icon(Icons.arrow_drop_down, color: tok.muted, size: 16),
            ],
          ),
        ),
      ),
    );
  }
}

class _ConnectionBadge extends StatefulWidget {
  const _ConnectionBadge({required this.status, required this.tok});

  final String status;
  final _Tok tok;

  @override
  State<_ConnectionBadge> createState() => _ConnectionBadgeState();
}

class _ConnectionBadgeState extends State<_ConnectionBadge>
    with SingleTickerProviderStateMixin {
  late final AnimationController _pulse;

  @override
  void initState() {
    super.initState();
    _pulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2200),
    )..repeat();
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final isLive = widget.status == 'connected' ||
        widget.status == 'streaming' ||
        widget.status == 'complete';
    final dotColor = isLive ? tok.cool : tok.muted;

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          AnimatedBuilder(
            animation: _pulse,
            builder: (_, __) {
              final t = _pulse.value * math.pi * 2;
              final opacity = (math.sin(t) * 0.34 + 0.66).clamp(0.32, 1.0);
              return Opacity(
                opacity: isLive ? opacity : 0.45,
                child: Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: dotColor,
                  ),
                ),
              );
            },
          ),
          const SizedBox(width: 5),
          Text(
            _label(widget.status),
            style: TextStyle(
              color: tok.muted,
              fontSize: 11,
              fontWeight: FontWeight.w700,
            ),
          ),
        ],
      ),
    );
  }

  static String _label(String s) => switch (s) {
        'idle' => '未連線',
        'connecting' => '連線中',
        'connected' => '已連線',
        'streaming' => '回應中',
        'reconnecting' => '重連中',
        'offline' => '離線',
        'complete' => '已連線',
        _ => s,
      };
}

class _SystemLine extends StatelessWidget {
  const _SystemLine({required this.tok, required this.text});

  final _Tok tok;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: Center(
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
          decoration: BoxDecoration(
            color: tok.surface.withOpacity(0.6),
            border: Border.all(color: tok.border.withOpacity(0.7)),
            borderRadius: BorderRadius.circular(999),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 6,
                height: 6,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: tok.cool,
                ),
              ),
              const SizedBox(width: 7),
              Text(
                text,
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _RoundLabel extends StatelessWidget {
  const _RoundLabel({required this.tok, required this.index});

  final _Tok tok;
  final int index;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Text(
          '回合 $index',
          style: TextStyle(
            color: tok.muted,
            fontSize: 11,
            fontWeight: FontWeight.w800,
          ),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Container(
            height: 0.5,
            color: tok.border.withOpacity(0.7),
          ),
        ),
      ],
    );
  }
}

class _UserBubble extends StatelessWidget {
  const _UserBubble({
    required this.tok,
    required this.text,
    required this.accent,
  });

  final _Tok tok;
  final String text;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        constraints: const BoxConstraints(maxWidth: 280),
        padding: const EdgeInsets.fromLTRB(13, 10, 13, 10),
        decoration: BoxDecoration(
          color: accent,
          borderRadius: const BorderRadius.only(
            topLeft: Radius.circular(15),
            topRight: Radius.circular(15),
            bottomLeft: Radius.circular(15),
            bottomRight: Radius.circular(5),
          ),
        ),
        child: Text(
          text,
          style: TextStyle(
            color: _textOn(accent),
            fontSize: 14,
            fontWeight: FontWeight.w500,
            height: 1.5,
          ),
        ),
      ),
    );
  }
}

class _MikuBubble extends StatelessWidget {
  const _MikuBubble({
    required this.tok,
    required this.text,
    required this.accent,
    required this.resources,
    required this.onOpenResource,
    this.isStreaming = false,
  });

  final _Tok tok;
  final String text;
  final Color accent;
  final List<String> resources;
  final void Function(String) onOpenResource;
  final bool isStreaming;

  @override
  Widget build(BuildContext context) {
    final iconColor = _textOn(accent);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            color: accent,
            borderRadius: BorderRadius.circular(9),
          ),
          child: Icon(Icons.smart_toy, color: iconColor, size: 17),
        ),
        const SizedBox(width: 9),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Text(
                    'Miku',
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 14,
                      fontWeight: FontWeight.w800,
                      letterSpacing: -0.3,
                    ),
                  ),
                  if (isStreaming) ...[
                    const SizedBox(width: 6),
                    _PulsingDot(color: accent),
                  ],
                ],
              ),
              const SizedBox(height: 5),
              Text(
                text,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 14,
                  fontWeight: FontWeight.w400,
                  height: 1.62,
                ),
              ),
              if (resources.isNotEmpty) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: resources
                      .map(
                        (uri) => GestureDetector(
                          onTap: () => onOpenResource(uri),
                          child: Container(
                            padding: const EdgeInsets.symmetric(
                                horizontal: 10, vertical: 6),
                            decoration: BoxDecoration(
                              color: tok.surface,
                              border: Border.all(color: tok.border),
                              borderRadius: BorderRadius.circular(9),
                            ),
                            child: Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                Icon(Icons.insert_drive_file_outlined,
                                    size: 13, color: accent),
                                const SizedBox(width: 6),
                                Text(
                                  uri,
                                  style: TextStyle(
                                    color: accent,
                                    fontSize: 11.5,
                                    fontWeight: FontWeight.w700,
                                    fontFamily: 'monospace',
                                  ),
                                ),
                                const SizedBox(width: 4),
                                Icon(Icons.open_in_new,
                                    size: 11, color: tok.muted),
                              ],
                            ),
                          ),
                        ),
                      )
                      .toList(),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

class _PulsingDot extends StatefulWidget {
  const _PulsingDot({required this.color});

  final Color color;

  @override
  State<_PulsingDot> createState() => _PulsingDotState();
}

class _PulsingDotState extends State<_PulsingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _c;

  @override
  void initState() {
    super.initState();
    _c = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1800),
    )..repeat();
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (_, __) {
        final opacity =
            (math.sin(_c.value * math.pi * 2) * 0.34 + 0.66).clamp(0.32, 1.0);
        return Opacity(
          opacity: opacity,
          child: Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: widget.color,
            ),
          ),
        );
      },
    );
  }
}

class _TypingIndicator extends StatelessWidget {
  const _TypingIndicator({
    required this.tok,
    required this.accent,
    required this.anim,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;

  @override
  Widget build(BuildContext context) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            color: accent,
            borderRadius: BorderRadius.circular(9),
          ),
          child: Icon(Icons.smart_toy, color: _textOn(accent), size: 17),
        ),
        const SizedBox(width: 9),
        Padding(
          padding: const EdgeInsets.only(top: 7),
          child: AnimatedBuilder(
            animation: anim,
            builder: (_, __) => Row(
              mainAxisSize: MainAxisSize.min,
              children: List.generate(3, (i) {
                final phase = (anim.value - i * 0.18) % 1.0;
                final opacity = (math.sin(phase * math.pi * 2) * 0.4 + 0.6)
                    .clamp(0.25, 1.0);
                final dy =
                    (math.sin(phase * math.pi * 2) * -2.0).clamp(-2.0, 0.0);
                return Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 2),
                  child: Transform.translate(
                    offset: Offset(0, dy),
                    child: Opacity(
                      opacity: opacity,
                      child: Container(
                        width: 6,
                        height: 6,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: tok.muted,
                        ),
                      ),
                    ),
                  ),
                );
              }),
            ),
          ),
        ),
      ],
    );
  }
}

class _AgentStatusBar extends StatelessWidget {
  const _AgentStatusBar({
    required this.tok,
    required this.accent,
    required this.anim,
    required this.agents,
    required this.activities,
    required this.onTap,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;
  final List<_AgentStatus> agents;
  final List<_ActivityItem> activities;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final running = agents.where((agent) => agent.isRunning).length;
    final stopped = agents.length - running;
    final visibleAgents =
        agents.length > 4 ? agents.sublist(agents.length - 4) : agents;
    final fallback = _runtimeFallback();
    return GestureDetector(
      onTap: onTap,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 30,
            height: 30,
            decoration: BoxDecoration(
              color: tok.surface,
              border: Border.all(color: tok.border),
              borderRadius: BorderRadius.circular(9),
            ),
            child: Icon(Icons.account_tree_outlined, color: accent, size: 16),
          ),
          const SizedBox(width: 9),
          Expanded(
            child: Container(
              padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
              decoration: BoxDecoration(
                color: tok.surface.withOpacity(0.78),
                border: Border.all(color: tok.border.withOpacity(0.82)),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Icon(Icons.route_outlined, color: accent, size: 14),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          agents.isEmpty
                              ? 'Runtime 狀態'
                              : 'Agents · $running running / $stopped stopped',
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: tok.text,
                            fontSize: 12,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                      ),
                      const SizedBox(width: 8),
                      Text(
                        '${activities.length} events',
                        style: TextStyle(
                          color: tok.muted,
                          fontSize: 10.5,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(width: 4),
                      Icon(Icons.open_in_full, color: tok.muted, size: 12),
                    ],
                  ),
                  const SizedBox(height: 7),
                  if (visibleAgents.isNotEmpty)
                    for (final agent in visibleAgents) ...[
                      _AgentStatusLine(
                        tok: tok,
                        accent: accent,
                        anim: anim,
                        agent: agent,
                      ),
                      if (agent != visibleAgents.last)
                        const SizedBox(height: 5),
                    ]
                  else
                    _RuntimeStatusLine(
                      tok: tok,
                      accent: accent,
                      anim: anim,
                      item: fallback,
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  _ActivityItem _runtimeFallback() {
    for (final item in activities.reversed) {
      if (item.kind == 'cell' || item.kind == 'tool') return item;
    }
    return activities.last;
  }
}

class _AgentStatusLine extends StatelessWidget {
  const _AgentStatusLine({
    required this.tok,
    required this.accent,
    required this.anim,
    required this.agent,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;
  final _AgentStatus agent;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        _StatusGlyph(tok: tok, accent: accent, anim: anim, state: agent.state),
        const SizedBox(width: 7),
        Expanded(
          child: Text(
            '${agent.role} agent · ${agent.id}',
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: tok.text,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
        ),
        Text(
          _stateLabel(agent.state),
          style: TextStyle(
            color: agent.isRunning ? accent : tok.muted,
            fontSize: 10.5,
            fontWeight: FontWeight.w800,
          ),
        ),
      ],
    );
  }
}

class _RuntimeStatusLine extends StatelessWidget {
  const _RuntimeStatusLine({
    required this.tok,
    required this.accent,
    required this.anim,
    required this.item,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;
  final _ActivityItem item;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        _StatusGlyph(tok: tok, accent: accent, anim: anim, state: item.state),
        const SizedBox(width: 7),
        Expanded(
          child: Text(
            item.title,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: tok.text,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
        ),
        Text(
          _stateLabel(item.state),
          style: TextStyle(
            color: item.state == _ActivityState.running ? accent : tok.muted,
            fontSize: 10.5,
            fontWeight: FontWeight.w800,
          ),
        ),
      ],
    );
  }
}

class _StatusGlyph extends StatelessWidget {
  const _StatusGlyph({
    required this.tok,
    required this.accent,
    required this.anim,
    required this.state,
  });

  final _Tok tok;
  final Color accent;
  final AnimationController anim;
  final _ActivityState state;

  @override
  Widget build(BuildContext context) {
    if (state == _ActivityState.running) {
      return AnimatedBuilder(
        animation: anim,
        builder: (_, __) {
          final opacity = (math.sin(anim.value * math.pi * 2) * 0.34 + 0.66)
              .clamp(0.34, 1.0);
          return Opacity(
            opacity: opacity,
            child: Container(
              width: 14,
              height: 14,
              decoration: BoxDecoration(
                color: accent,
                shape: BoxShape.circle,
              ),
            ),
          );
        },
      );
    }
    final icon = switch (state) {
      _ActivityState.failed => Icons.error_outline,
      _ActivityState.done => Icons.stop_circle_outlined,
      _ActivityState.info => Icons.info_outline,
      _ActivityState.running => Icons.circle,
    };
    final color = switch (state) {
      _ActivityState.failed => const Color(0xFFB84A30),
      _ActivityState.done => tok.muted,
      _ActivityState.info => tok.muted,
      _ActivityState.running => accent,
    };
    return Icon(icon, color: color, size: 15);
  }
}

class _ActivityRow extends StatelessWidget {
  const _ActivityRow({
    required this.tok,
    required this.accent,
    required this.item,
  });

  final _Tok tok;
  final Color accent;
  final _ActivityItem item;

  @override
  Widget build(BuildContext context) {
    final stateColor = _stateColor(item.state);
    final detail = _trimActivityDetail(item.detail);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 22,
          height: 22,
          decoration: BoxDecoration(
            color: stateColor.withOpacity(0.12),
            border: Border.all(color: stateColor.withOpacity(0.38)),
            borderRadius: BorderRadius.circular(7),
          ),
          child: Icon(item.icon, size: 13, color: stateColor),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                item.title,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 12.2,
                  fontWeight: FontWeight.w800,
                ),
              ),
              if (detail.isNotEmpty) ...[
                const SizedBox(height: 3),
                Text(
                  detail,
                  maxLines: item.monospace ? 5 : 3,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: item.monospace ? 11 : 11.5,
                    fontWeight: FontWeight.w600,
                    height: 1.35,
                    fontFamily: item.monospace ? 'monospace' : null,
                  ),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Color _stateColor(_ActivityState state) => switch (state) {
        _ActivityState.running => accent,
        _ActivityState.done => tok.cool,
        _ActivityState.failed => const Color(0xFFB84A30),
        _ActivityState.info => tok.muted,
      };
}

String _stateLabel(_ActivityState state) => switch (state) {
      _ActivityState.running => 'running',
      _ActivityState.done => 'stopped',
      _ActivityState.failed => 'failed',
      _ActivityState.info => 'updated',
    };

String _trimActivityDetail(String detail) {
  final cleaned = detail
      .trim()
      .split('\n')
      .map((line) => line.trimRight())
      .where((line) => line.trim().isNotEmpty)
      .join('\n');
  if (cleaned.length <= 520) return cleaned;
  return '${cleaned.substring(0, 517)}...';
}

class _ApprovalCard extends StatelessWidget {
  const _ApprovalCard({
    required this.tok,
    required this.approval,
    required this.accent,
    required this.onTap,
  });

  final _Tok tok;
  final ApprovalPrompt approval;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
        decoration: BoxDecoration(
          color: accent.withOpacity(0.09),
          border: Border.all(color: accent.withOpacity(0.45)),
          borderRadius: BorderRadius.circular(13),
        ),
        child: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                color: accent,
                borderRadius: BorderRadius.circular(9),
              ),
              child: Icon(
                Icons.warning_amber_rounded,
                color: _textOn(accent),
                size: 18,
              ),
            ),
            const SizedBox(width: 11),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    '待核可 · ${approval.action}',
                    style: TextStyle(
                      color: tok.text,
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    '點擊檢視詳情 · 逾時自動拒絕',
                    style: TextStyle(
                      color: tok.muted,
                      fontSize: 11.5,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
              ),
            ),
            Icon(Icons.chevron_right, color: tok.muted, size: 18),
          ],
        ),
      ),
    );
  }
}

class _MemoryProposalCard extends StatelessWidget {
  const _MemoryProposalCard({
    required this.tok,
    required this.proposal,
    required this.approval,
    required this.accent,
    required this.onApprove,
    required this.onDeny,
  });

  final _Tok tok;
  final MemoryWriteProposal proposal;
  final ApprovalPrompt? approval;
  final Color accent;
  final VoidCallback? onApprove;
  final VoidCallback? onDeny;

  @override
  Widget build(BuildContext context) {
    final hasApproval = approval != null;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
      decoration: BoxDecoration(
        color: tok.surface,
        border: Border.all(color: accent.withOpacity(0.46)),
        borderRadius: BorderRadius.circular(13),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: accent,
              borderRadius: BorderRadius.circular(9),
            ),
            child: Icon(Icons.memory, color: _textOn(accent), size: 18),
          ),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Memory proposal',
                        style: TextStyle(
                          color: tok.text,
                          fontSize: 12.8,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 7,
                        vertical: 2,
                      ),
                      decoration: BoxDecoration(
                        color: accent.withOpacity(0.1),
                        borderRadius: BorderRadius.circular(999),
                      ),
                      child: Text(
                        hasApproval ? 'pending' : 'syncing',
                        style: TextStyle(
                          color: accent,
                          fontSize: 10.5,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 5),
                Text(
                  proposal.displayText,
                  maxLines: 3,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: tok.text,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    height: 1.42,
                  ),
                ),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.label_outline,
                      text: proposal.kindLabel,
                    ),
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.folder_outlined,
                      text: 'scope ${proposal.scopeLabel}',
                    ),
                    _ProposalChip(
                      tok: tok,
                      icon: Icons.history,
                      text: 'provenance ${proposal.provenanceText}',
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                Row(
                  children: [
                    _ProposalActionButton(
                      tok: tok,
                      icon: Icons.close,
                      label: 'Deny',
                      onTap: onDeny,
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: _ProposalActionButton(
                        tok: tok,
                        icon: Icons.check,
                        label: 'Save memory',
                        accent: accent,
                        onTap: onApprove,
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _ProposalChip extends StatelessWidget {
  const _ProposalChip({
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
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: tok.bg,
        border: Border.all(color: tok.border),
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 12, color: tok.muted),
          const SizedBox(width: 5),
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 210),
            child: Text(
              text,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: tok.muted,
                fontSize: 10.8,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ProposalActionButton extends StatelessWidget {
  const _ProposalActionButton({
    required this.tok,
    required this.icon,
    required this.label,
    required this.onTap,
    this.accent,
  });

  final _Tok tok;
  final IconData icon;
  final String label;
  final VoidCallback? onTap;
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final active = onTap != null;
    final bg = accent ?? tok.bg;
    final fg = accent == null ? tok.text : _textOn(accent!);
    final border = accent ?? tok.border;
    return Opacity(
      opacity: active ? 1 : 0.54,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          height: 36,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: bg,
            border: Border.all(color: border),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Center(
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(icon, color: fg, size: 15),
                const SizedBox(width: 6),
                Flexible(
                  child: Text(
                    active ? label : 'Waiting',
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: fg,
                      fontSize: 12.3,
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
