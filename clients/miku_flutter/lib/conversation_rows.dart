part of 'conversation_screen.dart';

class _PresenceBar extends StatelessWidget {
  const _PresenceBar({
    required this.connection,
    required this.session,
    required this.onOpenDrawer,
    required this.onOpenContext,
  });

  final _ServerConnectionState connection;
  final MikuSession? session;
  final VoidCallback onOpenDrawer;
  final VoidCallback onOpenContext;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final status = switch (connection) {
      _ServerConnectionState.connecting => '正在連上伺服器',
      _ServerConnectionState.connected => '伺服器已連線',
      _ServerConnectionState.reconnecting => '正在重新連線',
      _ServerConnectionState.offline => '伺服器未連線',
    };
    return Semantics(
      container: true,
      liveRegion: true,
      label: status,
      child: SizedBox(
        height: 68,
        child: Row(
          children: [
            IconButton(
              key: const Key('open-left-drawer'),
              tooltip: '開啟對話選單',
              onPressed: onOpenDrawer,
              icon: const Icon(Icons.menu_rounded),
            ),
            const SizedBox(width: 4),
            _PresenceMark(
              active: connection == _ServerConnectionState.connected,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    'Miku',
                    style: Theme.of(context).textTheme.titleMedium?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 1),
                  Text(
                    status,
                    key: const Key('presence-status'),
                    style: Theme.of(
                      context,
                    ).textTheme.bodySmall?.copyWith(color: palette.muted),
                  ),
                ],
              ),
            ),
            IconButton(
              key: const Key('open-session-context'),
              tooltip: '開啟對話狀態',
              onPressed: session == null ? null : onOpenContext,
              icon: const Icon(Icons.tune_rounded),
            ),
          ],
        ),
      ),
    );
  }
}

class _PresenceMark extends StatelessWidget {
  const _PresenceMark({required this.active});

  final bool active;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Container(
      key: const Key('miku-presence-mark'),
      width: 34,
      height: 34,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: palette.miku.withValues(alpha: active ? 0.16 : 0.07),
        border: Border.all(
          color: palette.miku.withValues(alpha: active ? 0.7 : 0.25),
        ),
      ),
      alignment: Alignment.center,
      child: Container(
        width: 9,
        height: 9,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: active ? palette.miku : palette.muted,
        ),
      ),
    );
  }
}

class _MessageRow extends StatelessWidget {
  const _MessageRow({required this.message});

  final _MessageItem message;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final user = message.role == 'user';
    final body =
        user
            ? SelectableText(
              message.text,
              key: Key('message-${message.key}'),
              style: Theme.of(context).textTheme.bodyLarge,
            )
            : MikuRichMessage(
              key: Key('message-${message.key}'),
              data: message.text,
            );
    return Semantics(
      liveRegion: false,
      label: user ? '你說：${message.text}' : null,
      child: Align(
        alignment: user ? Alignment.centerRight : Alignment.centerLeft,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: user ? 560 : 690),
          child:
              user
                  ? DecoratedBox(
                    decoration: BoxDecoration(
                      color: palette.userBubble,
                      borderRadius: const BorderRadius.only(
                        topLeft: Radius.circular(20),
                        topRight: Radius.circular(7),
                        bottomLeft: Radius.circular(20),
                        bottomRight: Radius.circular(20),
                      ),
                      border: Border.all(color: palette.outline),
                    ),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 16,
                        vertical: 11,
                      ),
                      child: body,
                    ),
                  )
                  : Padding(
                    padding: const EdgeInsets.only(left: 3, right: 16),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.end,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Flexible(child: body),
                        if (message.streaming) ...[
                          const SizedBox(width: 7),
                          _StreamingDot(color: palette.miku),
                        ],
                      ],
                    ),
                  ),
        ),
      ),
    );
  }
}

class _StreamingDot extends StatefulWidget {
  const _StreamingDot({required this.color});

  final Color color;

  @override
  State<_StreamingDot> createState() => _StreamingDotState();
}

class _StreamingDotState extends State<_StreamingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 900),
      lowerBound: 0.35,
      upperBound: 1,
    )..repeat(reverse: true);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context)) {
      _controller.stop();
    } else if (!_controller.isAnimating) {
      _controller.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return FadeTransition(
      opacity: _controller,
      child: Container(
        width: 6,
        height: 6,
        decoration: BoxDecoration(color: widget.color, shape: BoxShape.circle),
      ),
    );
  }
}

class _TurnStatusRow extends StatelessWidget {
  const _TurnStatusRow({required this.turn});

  final _TurnItem turn;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final failed = const {
      'failed',
      'cancelled',
      'timed_out',
    }.contains(turn.status);
    final complete = turn.status == 'completed';
    final label = switch (turn.status) {
      'submitting' => '正在送到伺服器',
      'queued' => '已排入安全佇列',
      'running' => 'Miku 正在處理',
      'waiting' => '等待你的確認',
      'finalizing' => '回覆已收到，正在確認保存',
      'completed' => '已完成並保存',
      'failed' => '處理失敗',
      'cancelled' => '已取消',
      'timed_out' => '處理逾時',
      _ => '伺服器狀態：${turn.status}',
    };
    final color = failed ? Theme.of(context).colorScheme.error : palette.muted;
    return Semantics(
      liveRegion: !turn.isTerminal,
      label: '$label${turn.error == null ? '' : '，${turn.error}'}',
      child: Align(
        alignment: Alignment.centerRight,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 560),
          child: Padding(
            padding: const EdgeInsets.only(right: 4),
            child: Row(
              key: Key('turn-status-${turn.key}'),
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Padding(
                  padding: const EdgeInsets.only(top: 2),
                  child:
                      complete
                          ? Icon(
                            Icons.cloud_done_outlined,
                            size: 15,
                            color: palette.miku,
                          )
                          : failed
                          ? Icon(
                            Icons.error_outline_rounded,
                            size: 15,
                            color: color,
                          )
                          : SizedBox.square(
                            dimension: 13,
                            child: CircularProgressIndicator(
                              strokeWidth: 1.5,
                              color: palette.miku,
                            ),
                          ),
                ),
                const SizedBox(width: 7),
                Flexible(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        label,
                        style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color: color,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                      if (turn.error != null && turn.error!.trim().isNotEmpty)
                        Text(
                          turn.error!,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(
                            context,
                          ).textTheme.bodySmall?.copyWith(color: color),
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
}

class _ActivityRow extends StatelessWidget {
  const _ActivityRow({required this.activity, required this.onOpenResource});

  final _ActivityItem activity;
  final ValueChanged<String> onOpenResource;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      key: Key('activity-${activity.correlationKey ?? activity.key}'),
      liveRegion: activity.running || activity.phase == _ActivityPhase.paused,
      label:
          '${activity.label}${activity.detail == null ? '' : '，${activity.detail}'}',
      child: Padding(
        padding: const EdgeInsets.only(left: 3),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: SizedBox(
                width: 12,
                height: 12,
                child: _ActivityStatusMark(activity: activity),
              ),
            ),
            const SizedBox(width: 9),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    activity.label,
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: palette.muted,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  if (activity.detail != null)
                    Text(
                      activity.detail!,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                        color: palette.muted.withValues(alpha: 0.78),
                      ),
                    ),
                  if (activity.links.isNotEmpty) ...[
                    const SizedBox(height: 3),
                    Wrap(
                      spacing: 4,
                      runSpacing: 2,
                      children: [
                        for (final link in activity.links)
                          TextButton.icon(
                            key: Key(
                              'activity-resource-${link.kind}-${link.uri}',
                            ),
                            style: TextButton.styleFrom(
                              minimumSize: const Size(0, 44),
                              padding: const EdgeInsets.symmetric(
                                horizontal: 8,
                              ),
                              tapTargetSize: MaterialTapTargetSize.padded,
                              visualDensity: VisualDensity.standard,
                            ),
                            onPressed: () => onOpenResource(link.uri),
                            icon: Icon(link.icon, size: 17),
                            label: Text(link.label),
                          ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ActivityGroupRow extends StatelessWidget {
  const _ActivityGroupRow({
    required this.group,
    required this.expanded,
    required this.onToggle,
    required this.onOpenResource,
  });

  final _ActivityGroupNode group;
  final bool expanded;
  final VoidCallback onToggle;
  final ValueChanged<String> onOpenResource;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final activities = group.activities;
    final active = group.hasActive;
    final anyFailed = activities.any(
      (a) =>
          a.phase == _ActivityPhase.failed ||
          a.phase == _ActivityPhase.cancelled,
    );
    final current = activities.lastWhere(
      (a) =>
          a.phase == _ActivityPhase.running || a.phase == _ActivityPhase.paused,
      orElse: () => activities.last,
    );
    final headerLabel =
        active
            ? current.label
            : anyFailed
            ? '想一想過程（部分未完成）'
            : '想一想過程';
    final Widget mark;
    if (active) {
      mark = CircularProgressIndicator(strokeWidth: 1.6, color: palette.miku);
    } else if (anyFailed) {
      mark = Icon(
        Icons.error_outline_rounded,
        size: 13,
        color: Theme.of(context).colorScheme.error,
      );
    } else {
      mark = Icon(Icons.check_rounded, size: 13, color: palette.miku);
    }
    return Semantics(
      key: Key('activity-group-${group.key}'),
      button: true,
      liveRegion: active,
      label:
          '$headerLabel，${activities.length} 個步驟，${expanded ? '已展開' : '已收合'}',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          InkWell(
            key: Key('activity-group-toggle-${group.key}'),
            onTap: onToggle,
            borderRadius: BorderRadius.circular(8),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 10, horizontal: 3),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.center,
                children: [
                  SizedBox(width: 12, height: 12, child: mark),
                  const SizedBox(width: 9),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          headerLabel,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(
                            context,
                          ).textTheme.bodySmall?.copyWith(
                            color: palette.muted,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                        Text(
                          '${activities.length} 個步驟',
                          style: Theme.of(
                            context,
                          ).textTheme.bodySmall?.copyWith(
                            color: palette.muted.withValues(alpha: 0.78),
                          ),
                        ),
                      ],
                    ),
                  ),
                  Icon(
                    expanded
                        ? Icons.expand_less_rounded
                        : Icons.expand_more_rounded,
                    size: 18,
                    color: palette.muted,
                  ),
                ],
              ),
            ),
          ),
          if (expanded)
            Padding(
              padding: const EdgeInsets.only(left: 12, top: 6),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  for (var i = 0; i < activities.length; i++) ...[
                    if (i > 0) const SizedBox(height: 12),
                    _ActivityRow(
                      activity: activities[i],
                      onOpenResource: onOpenResource,
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

class _InlineNotice extends StatelessWidget {
  const _InlineNotice({required this.notice});

  final _NoticeItem notice;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      liveRegion: notice.isError,
      child: Text(
        notice.text,
        style: Theme.of(context).textTheme.bodySmall?.copyWith(
          color:
              notice.isError
                  ? Theme.of(context).colorScheme.error
                  : palette.muted,
        ),
      ),
    );
  }
}

class _ConnectionNotice extends StatelessWidget {
  const _ConnectionNotice({
    required this.text,
    required this.canRetry,
    required this.onRetry,
    this.onPair,
  });

  final String text;
  final bool canRetry;
  final VoidCallback onRetry;
  final VoidCallback? onPair;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      liveRegion: true,
      child: Padding(
        padding: const EdgeInsets.only(top: 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              text,
              key: const Key('connection-notice'),
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                color: Theme.of(context).colorScheme.error,
              ),
            ),
            if (canRetry || onPair != null)
              Align(
                alignment: AlignmentDirectional.centerEnd,
                child: Wrap(
                  alignment: WrapAlignment.end,
                  spacing: 4,
                  children: [
                    if (canRetry)
                      TextButton(
                        key: const Key('retry-connection'),
                        onPressed: onRetry,
                        child: const Text('重新連線'),
                      ),
                    if (onPair != null)
                      FilledButton.tonalIcon(
                        key: const Key('open-pairing-settings'),
                        onPressed: onPair,
                        icon: const Icon(Icons.link_rounded),
                        label: const Text('設定與配對'),
                      ),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _QuietLoading extends StatelessWidget {
  const _QuietLoading();

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return Semantics(
      liveRegion: true,
      label: '正在載入對話',
      child: SizedBox.square(
        dimension: 24,
        child: CircularProgressIndicator(strokeWidth: 2, color: palette.miku),
      ),
    );
  }
}
