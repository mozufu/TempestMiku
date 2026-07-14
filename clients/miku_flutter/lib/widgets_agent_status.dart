part of 'main.dart';

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
    if (_reducedMotion(context)) {
      return Container(
        width: 6,
        height: 6,
        decoration: BoxDecoration(shape: BoxShape.circle, color: widget.color),
      );
    }
    return AnimatedBuilder(
      animation: _c,
      builder: (_, __) {
        final opacity = (math.sin(_c.value * math.pi * 2) * 0.34 + 0.66).clamp(
          0.32,
          1.0,
        );
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
    final reduceMotion = _reducedMotion(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 34,
          height: 34,
          padding: const EdgeInsets.all(7),
          decoration: BoxDecoration(
            color: tok.text,
            borderRadius: BorderRadius.circular(11),
          ),
          child: const MikuStormCatMark(
            color: Color(0xFF39C5BB),
            boltColor: Color(0xFFFF7B70),
          ),
        ),
        const SizedBox(width: 9),
        Padding(
          padding: const EdgeInsets.only(top: 7),
          child: AnimatedBuilder(
            animation: anim,
            builder:
                (_, __) => Row(
                  mainAxisSize: MainAxisSize.min,
                  children: List.generate(3, (i) {
                    final phase =
                        reduceMotion ? 0.0 : (anim.value - i * 0.18) % 1.0;
                    final opacity =
                        reduceMotion
                            ? 0.85
                            : (math.sin(phase * math.pi * 2) * 0.4 + 0.6).clamp(
                              0.25,
                              1.0,
                            );
                    final dy =
                        reduceMotion
                            ? 0.0
                            : (math.sin(phase * math.pi * 2) * -2.0).clamp(
                              -2.0,
                              0.0,
                            );
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
    required this.copy,
    required this.accent,
    required this.anim,
    required this.roundIndex,
    required this.agents,
    required this.activities,
    required this.expanded,
    required this.onTap,
    required this.onOpenResource,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final AnimationController anim;
  final int roundIndex;
  final List<_AgentStatus> agents;
  final List<_ActivityItem> activities;
  final bool expanded;
  final VoidCallback onTap;
  final void Function(String) onOpenResource;

  @override
  Widget build(BuildContext context) {
    final running = agents.where((agent) => agent.isRunning).length;
    final stopped = agents.length - running;
    final visibleAgents =
        agents.length > 4 ? agents.sublist(agents.length - 4) : agents;
    final fallback = _runtimeFallback();
    return Semantics(
      button: true,
      label: copy.openAgentActivity,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          key: ValueKey('agent-activity:$roundIndex'),
          onTap: onTap,
          borderRadius: BorderRadius.circular(10),
          focusColor: tok.focus.withValues(alpha: 0.18),
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
                child: Icon(
                  Icons.account_tree_outlined,
                  color: accent,
                  size: 16,
                ),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Container(
                  padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
                  decoration: BoxDecoration(
                    color: tok.surface.withValues(alpha: 0.78),
                    border: Border.all(
                      color: tok.border.withValues(alpha: 0.82),
                    ),
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
                                  ? copy.runtimeStatus
                                  : copy.agentsSummary(running, stopped),
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
                            copy.events(activities.length),
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
                            copy: copy,
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
                          copy: copy,
                          accent: accent,
                          anim: anim,
                          item: fallback,
                        ),
                      if (expanded) ...[
                        const SizedBox(height: 9),
                        Container(
                          height: 0.5,
                          color: tok.border.withValues(alpha: 0.72),
                        ),
                        const SizedBox(height: 9),
                        for (var i = 0; i < activities.length; i++) ...[
                          _ActivityRow(
                            tok: tok,
                            copy: copy,
                            accent: accent,
                            item: activities[i],
                            onOpenResource: onOpenResource,
                          ),
                          if (i != activities.length - 1) ...[
                            const SizedBox(height: 8),
                            Container(
                              height: 0.5,
                              color: tok.border.withValues(alpha: 0.55),
                            ),
                            const SizedBox(height: 8),
                          ],
                        ],
                      ],
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
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

// ─── Reasoning / chain-of-thought trace ───────────────────────────────────────

/// A collapsible rendering of the private chain-of-thought a provider returned
/// alongside the answer. The parent round controls whether it stays expanded.
class _ThinkingTrace extends StatefulWidget {
  const _ThinkingTrace({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.text,
    required this.expanded,
    required this.isStreaming,
  });

  final _Tok tok;
  final _UiCopy copy;
  final Color accent;
  final String text;
  final bool expanded;
  final bool isStreaming;

  @override
  State<_ThinkingTrace> createState() => _ThinkingTraceState();
}

class _ThinkingTraceState extends State<_ThinkingTrace> {
  late bool _expanded = widget.expanded;
  bool _userToggled = false;

  @override
  void didUpdateWidget(covariant _ThinkingTrace oldWidget) {
    super.didUpdateWidget(oldWidget);
    // Follow the parent's expanded state for live open/close, unless the user has
    // manually toggled this trace.
    if (!_userToggled && oldWidget.expanded != widget.expanded) {
      _expanded = widget.expanded;
    }
  }

  void _toggle() {
    setState(() {
      _expanded = !_expanded;
      _userToggled = true;
    });
  }

  @override
  Widget build(BuildContext context) {
    final tok = widget.tok;
    final accent = widget.accent;
    return Semantics(
      button: true,
      label: widget.copy.thinkingTrace,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: _toggle,
          borderRadius: BorderRadius.circular(10),
          focusColor: tok.focus.withValues(alpha: 0.18),
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
                child: Icon(Icons.psychology_outlined, color: accent, size: 16),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Container(
                  padding: const EdgeInsets.fromLTRB(11, 10, 11, 10),
                  decoration: BoxDecoration(
                    color: tok.surface.withValues(alpha: 0.78),
                    border: Border.all(
                      color: tok.border.withValues(alpha: 0.82),
                    ),
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Icon(
                            Icons.psychology_outlined,
                            color: accent,
                            size: 14,
                          ),
                          const SizedBox(width: 6),
                          Expanded(
                            child: Text(
                              widget.copy.thinking,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                color: tok.muted,
                                fontSize: 12,
                                fontWeight: FontWeight.w800,
                                letterSpacing: 0.2,
                              ),
                            ),
                          ),
                          if (widget.isStreaming) ...[
                            _PulsingDot(color: accent),
                            const SizedBox(width: 6),
                          ],
                          Icon(
                            _expanded ? Icons.expand_less : Icons.expand_more,
                            color: tok.muted,
                            size: 16,
                          ),
                        ],
                      ),
                      if (_expanded) ...[
                        const SizedBox(height: 8),
                        Container(
                          height: 0.5,
                          color: tok.border.withValues(alpha: 0.72),
                        ),
                        const SizedBox(height: 8),
                        Container(
                          constraints: const BoxConstraints(maxHeight: 240),
                          child: SingleChildScrollView(
                            child: SelectableText(
                              widget.text,
                              style: TextStyle(
                                color: tok.muted,
                                fontSize: 12.5,
                                height: 1.55,
                                fontFamily: 'monospace',
                                fontStyle: FontStyle.italic,
                              ),
                            ),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _AgentStatusLine extends StatelessWidget {
  const _AgentStatusLine({
    required this.tok,
    required this.copy,
    required this.accent,
    required this.anim,
    required this.agent,
  });

  final _Tok tok;
  final _UiCopy copy;
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
          copy.stateLabel(agent.state),
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
    required this.copy,
    required this.accent,
    required this.anim,
    required this.item,
  });

  final _Tok tok;
  final _UiCopy copy;
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
          copy.stateLabel(item.state),
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
      if (_reducedMotion(context)) {
        return Container(
          width: 14,
          height: 14,
          decoration: BoxDecoration(color: accent, shape: BoxShape.circle),
        );
      }
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
              decoration: BoxDecoration(color: accent, shape: BoxShape.circle),
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
      _ActivityState.failed => tok.danger,
      _ActivityState.done => tok.success,
      _ActivityState.info => tok.muted,
      _ActivityState.running => accent,
    };
    return Icon(icon, color: color, size: 15);
  }
}
