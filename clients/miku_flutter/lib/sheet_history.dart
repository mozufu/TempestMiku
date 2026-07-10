part of 'main.dart';

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
          focusColor: tok.focus.withValues(alpha: 0.18),
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 11, 12, 11),
            decoration: BoxDecoration(
              color: selected ? accent.withValues(alpha: 0.11) : tok.bg,
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
        color: tok.surface.withValues(alpha: 0.68),
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
                  color: Colors.black.withValues(alpha: 0.15),
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
