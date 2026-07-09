part of 'main.dart';

// ─── Small reusable widgets ────────────────────────────────────────────────────

bool _reducedMotion(BuildContext context) {
  final media = MediaQuery.maybeOf(context);
  return media?.disableAnimations == true ||
      media?.accessibleNavigation == true;
}

class _TokIconBtn extends StatelessWidget {
  const _TokIconBtn({
    required this.tok,
    required this.icon,
    required this.tooltip,
    required this.semanticLabel,
    required this.onTap,
  });

  final _Tok tok;
  final IconData icon;
  final String tooltip;
  final String semanticLabel;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: Semantics(
        button: true,
        enabled: onTap != null,
        label: semanticLabel,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(10),
            focusColor: tok.focus.withOpacity(0.2),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Icon(icon, color: tok.muted, size: 17),
            ),
          ),
        ),
      ),
    );
  }
}

class _ModeDropMenuButton extends StatelessWidget {
  const _ModeDropMenuButton({
    required this.tok,
    required this.copy,
    required this.mode,
    required this.accent,
    required this.locked,
    required this.onTap,
    this.compact = false,
  });

  final _Tok tok;
  final _UiCopy copy;
  final _Mode mode;
  final Color accent;
  final bool locked;
  final VoidCallback onTap;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final borderColor = locked ? accent.withOpacity(0.58) : tok.border;
    final bg = locked ? accent.withOpacity(0.12) : tok.surface.withOpacity(0.6);
    final label = copy.modeChipLabel(mode, locked);
    return Tooltip(
      message: locked ? copy.modeLocked : copy.switchMode,
      child: Semantics(
        button: true,
        label: locked ? copy.currentModeLocked(mode) : copy.currentMode(mode),
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(999),
            focusColor: tok.focus.withOpacity(0.18),
            child: Container(
              height: 40,
              constraints: BoxConstraints(
                minWidth: compact ? 62 : 72,
                maxWidth: compact ? 90 : 128,
              ),
              padding: const EdgeInsets.symmetric(horizontal: 10),
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
                    size: 14,
                  ),
                  const SizedBox(width: 5),
                  Flexible(
                    child: Text(
                      label,
                      overflow: TextOverflow.ellipsis,
                      maxLines: 1,
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
        ),
      ),
    );
  }
}

class _ConnectionBadge extends StatefulWidget {
  const _ConnectionBadge({
    required this.status,
    required this.tok,
    required this.copy,
    this.compact = false,
  });

  final String status;
  final _Tok tok;
  final _UiCopy copy;
  final bool compact;

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
    final dotColor = isLive ? tok.success : tok.warning;
    final label = widget.copy.statusLabel(widget.status);
    final reduceMotion = _reducedMotion(context);

    Widget dot() {
      return AnimatedBuilder(
        animation: _pulse,
        builder: (_, __) {
          final t = _pulse.value * math.pi * 2;
          final opacity =
              reduceMotion ? 1.0 : (math.sin(t) * 0.34 + 0.66).clamp(0.32, 1.0);
          return Opacity(
            opacity: isLive ? opacity : 0.55,
            child: Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: dotColor,
              ),
            ),
          );
        },
      );
    }

    return Tooltip(
      message: label,
      child: Semantics(
        label: widget.copy.connectionStatus(label),
        child: Container(
          width: widget.compact ? 40 : null,
          height: widget.compact ? 40 : null,
          padding: widget.compact
              ? EdgeInsets.zero
              : const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          decoration: BoxDecoration(
            border: Border.all(color: tok.border),
            borderRadius: BorderRadius.circular(999),
          ),
          child: widget.compact
              ? Center(child: dot())
              : Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    dot(),
                    const SizedBox(width: 6),
                    Text(
                      label,
                      style: TextStyle(
                        color: tok.muted,
                        fontSize: 11,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ],
                ),
        ),
      ),
    );
  }
}

class _LanguageToggle extends StatelessWidget {
  const _LanguageToggle({
    required this.tok,
    required this.copy,
    required this.onTap,
  });

  final _Tok tok;
  final _UiCopy copy;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: copy.languageTooltip,
      child: Semantics(
        button: true,
        label: copy.languageSemantic,
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(10),
            focusColor: tok.focus.withOpacity(0.2),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Center(
                child: Text(
                  copy.code,
                  style: TextStyle(
                    color: tok.muted,
                    fontSize: 11,
                    fontWeight: FontWeight.w900,
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState({
    required this.tok,
    required this.accent,
    required this.status,
    required this.copy,
  });

  final _Tok tok;
  final Color accent;
  final String status;
  final _UiCopy copy;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 92, 4, 28),
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 360),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(
                  color: accent,
                  borderRadius: BorderRadius.circular(12),
                ),
                child: Icon(Icons.smart_toy, color: _textOn(accent), size: 22),
              ),
              const SizedBox(height: 14),
              Text(
                copy.emptyTitle,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 16,
                  fontWeight: FontWeight.w800,
                ),
              ),
              const SizedBox(height: 6),
              Text(
                copy.statusLabel(status),
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: tok.muted,
                  fontSize: 12,
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
  const _RoundLabel({
    required this.tok,
    required this.copy,
    required this.index,
  });

  final _Tok tok;
  final _UiCopy copy;
  final int index;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Text(
          copy.round(index),
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
    required this.copy,
    required this.text,
    required this.accent,
    required this.resources,
    required this.onOpenResource,
    this.isStreaming = false,
  });

  final _Tok tok;
  final _UiCopy copy;
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
              _MarkdownMessage(
                tok: tok,
                accent: accent,
                text: text,
              ),
              if (resources.isNotEmpty) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: resources
                      .map(
                        (uri) => Semantics(
                          button: true,
                          label: copy.openResource(uri),
                          child: Material(
                            color: Colors.transparent,
                            child: InkWell(
                              key: ValueKey('resource:$uri'),
                              onTap: () => onOpenResource(uri),
                              borderRadius: BorderRadius.circular(9),
                              focusColor: tok.focus.withOpacity(0.18),
                              child: Container(
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 10,
                                  vertical: 7,
                                ),
                                decoration: BoxDecoration(
                                  color: tok.surface,
                                  border: Border.all(color: tok.border),
                                  borderRadius: BorderRadius.circular(9),
                                ),
                                child: Row(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    Icon(
                                      Icons.insert_drive_file_outlined,
                                      size: 13,
                                      color: accent,
                                    ),
                                    const SizedBox(width: 6),
                                    ConstrainedBox(
                                      constraints:
                                          const BoxConstraints(maxWidth: 260),
                                      child: Text(
                                        uri,
                                        maxLines: 1,
                                        overflow: TextOverflow.ellipsis,
                                        style: TextStyle(
                                          color: accent,
                                          fontSize: 11.5,
                                          fontWeight: FontWeight.w700,
                                          fontFamily: 'monospace',
                                        ),
                                      ),
                                    ),
                                    const SizedBox(width: 4),
                                    Icon(
                                      Icons.open_in_new,
                                      size: 11,
                                      color: tok.muted,
                                    ),
                                  ],
                                ),
                              ),
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
