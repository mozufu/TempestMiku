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
            focusColor: tok.focus.withValues(alpha: 0.2),
            child: Container(
              width: 48,
              height: 48,
              decoration: BoxDecoration(
                border: Border.all(color: tok.border),
                borderRadius: BorderRadius.circular(14),
              ),
              child: Icon(icon, color: tok.muted, size: 20),
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
    required this.status,
    required this.copy,
  });

  final _Tok tok;
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
              const MikuBrandBadge(size: 64),
              const SizedBox(height: 18),
              Text(
                copy.emptyTitle,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: tok.text,
                  fontSize: 22,
                  fontWeight: FontWeight.w900,
                  letterSpacing: -0.45,
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
        constraints: const BoxConstraints(maxWidth: 560),
        padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
        decoration: BoxDecoration(
          color: accent,
          borderRadius: const BorderRadius.only(
            topLeft: Radius.circular(20),
            topRight: Radius.circular(20),
            bottomLeft: Radius.circular(20),
            bottomRight: Radius.circular(7),
          ),
        ),
        child: Text(
          text,
          style: TextStyle(
            color: _textOn(accent),
            fontSize: 15,
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
            boxShadow: [BoxShadow(color: tok.glow, blurRadius: 12)],
          ),
          child: const MikuStormCatMark(
            color: Color(0xFF39C5BB),
            boltColor: Color(0xFFFF7B70),
          ),
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
                      fontSize: 14.5,
                      fontWeight: FontWeight.w900,
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
              _MarkdownMessage(tok: tok, accent: accent, text: text),
              if (resources.isNotEmpty) ...[
                const SizedBox(height: 8),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children:
                      resources
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
                                  focusColor: tok.focus.withValues(alpha: 0.18),
                                  child: Container(
                                    constraints: const BoxConstraints(
                                      minHeight: 44,
                                    ),
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 12,
                                      vertical: 9,
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
                                          constraints: const BoxConstraints(
                                            maxWidth: 260,
                                          ),
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
