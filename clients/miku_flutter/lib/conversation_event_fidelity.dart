part of 'conversation_screen.dart';

enum _ActivityPhase { running, paused, completed, failed, cancelled }

class _ActivityResourceLink {
  const _ActivityResourceLink({required this.uri, required this.kind});

  final String uri;
  final String kind;

  String get label => switch (kind) {
    'history' => '查看歷程',
    'memory' => '查看回想',
    _ => '開啟產物',
  };

  IconData get icon => switch (kind) {
    'history' => Icons.history_rounded,
    'memory' => Icons.psychology_outlined,
    _ => Icons.description_outlined,
  };
}

class _ProposalItem extends _ConversationItem {
  _ProposalItem({
    required String key,
    required this.proposalId,
    required this.kind,
    required this.status,
  }) : super(key);

  final String proposalId;
  String kind;
  String status;
}

extension _ConversationEventFidelity on _ConversationScreenState {
  void _addActivity(
    MikuEvent event,
    String label, {
    String? detail,
    String? correlationKey,
    _ActivityPhase phase = _ActivityPhase.running,
    List<_ActivityResourceLink> links = const [],
  }) {
    final correlation = correlationKey ?? _activityCorrelationKey(event);
    final boundedDetail = _boundedEventText(detail);
    _ActivityItem? existing;
    if (correlation != null) {
      for (final item in _items.reversed.whereType<_ActivityItem>()) {
        if (item.correlationKey == correlation) {
          existing = item;
          break;
        }
      }
    }
    if (existing != null) {
      existing
        ..label = label
        ..phase = phase;
      if (boundedDetail != null) existing.detail = boundedDetail;
      _mergeActivityLinks(existing, links);
      return;
    }
    _items.add(
      _ActivityItem(
        key: event.id ?? _nextKey('activity'),
        correlationKey: correlation,
        label: label,
        detail: boundedDetail,
        phase: phase,
        links: links,
      ),
    );
  }

  void _completeActivity(
    MikuEvent event, {
    String? label,
    _ActivityPhase phase = _ActivityPhase.completed,
    List<_ActivityResourceLink> links = const [],
  }) {
    final correlation = _activityCorrelationKey(event);
    final result = _firstEventString(event.data, const [
      'resultPreview',
      'summary',
      'error',
      'reason',
    ]);
    if (correlation != null) {
      for (final item in _items.reversed.whereType<_ActivityItem>()) {
        if (item.correlationKey != correlation) continue;
        item.phase = phase;
        if (label != null) item.label = label;
        final detail = _boundedEventText(result);
        if (detail != null) item.detail = detail;
        _mergeActivityLinks(item, links);
        return;
      }
      _addActivity(
        event,
        label ?? _terminalActivityLabel(event.type, phase),
        detail: result,
        correlationKey: correlation,
        phase: phase,
        links: links,
      );
      return;
    }

    // Old servers did not always include correlation IDs. Only those genuinely
    // unkeyed events may use the legacy latest-running fallback.
    for (final item in _items.reversed.whereType<_ActivityItem>()) {
      if (!item.running) continue;
      item.phase = phase;
      if (label != null) item.label = label;
      final detail = _boundedEventText(result);
      if (detail != null) item.detail = detail;
      _mergeActivityLinks(item, links);
      return;
    }
  }

  void _pauseActivity(MikuEvent event) {
    final correlation = _activityCorrelationKey(event);
    if (correlation != null) {
      for (final item in _items.reversed.whereType<_ActivityItem>()) {
        if (item.correlationKey != correlation) continue;
        item
          ..phase = _ActivityPhase.paused
          ..label = '等待確認';
        return;
      }
      _addActivity(
        event,
        '等待確認',
        correlationKey: correlation,
        phase: _ActivityPhase.paused,
      );
      return;
    }
    for (final item in _items.reversed.whereType<_ActivityItem>()) {
      if (!item.running) continue;
      item
        ..phase = _ActivityPhase.paused
        ..label = '等待確認';
      return;
    }
  }

  void _resumeActivity(MikuEvent event) {
    final correlation = _activityCorrelationKey(event);
    if (correlation != null) {
      for (final item in _items.reversed.whereType<_ActivityItem>()) {
        if (item.correlationKey != correlation) continue;
        item
          ..phase = _ActivityPhase.running
          ..label = '繼續執行';
        return;
      }
      _addActivity(event, '繼續執行', correlationKey: correlation);
      return;
    }
    for (final item in _items.reversed.whereType<_ActivityItem>()) {
      if (item.phase != _ActivityPhase.paused) continue;
      item
        ..phase = _ActivityPhase.running
        ..label = '繼續執行';
      return;
    }
  }

  void _recordFidelityEvent(MikuEvent event) {
    switch (event.type) {
      case 'display':
        _addActivity(
          event,
          '已產生顯示結果',
          detail: _displayDetail(event),
          correlationKey: _eventSpecificKey('display', event),
          phase: _ActivityPhase.completed,
        );
      case 'binding_committed':
        final count = _firstEventString(event.data, const ['bindingCount']);
        _addActivity(
          event,
          '已保存執行結果',
          detail: count.isEmpty ? null : '$count 個綁定',
          correlationKey: _eventSpecificKey('binding', event),
          phase: _ActivityPhase.completed,
        );
      case 'scope_start':
        _addActivity(event, '正在平行處理', detail: _scopeProgress(event));
      case 'scope_progress':
        _addActivity(event, '正在平行處理', detail: _scopeProgress(event));
      case 'scope_result':
        final phase = _runtimeTerminalPhase(event);
        _completeActivity(
          event,
          label: switch (phase) {
            _ActivityPhase.completed => '平行工作完成',
            _ActivityPhase.cancelled => '平行工作已取消',
            _ => '平行工作未完成',
          },
          phase: phase,
        );
      case 'actor_status':
        _recordActorStatus(event);
      case 'actor_message':
        _addActivity(event, '分工代理正在協作', detail: '已傳遞一則內部訊息');
      case 'actor_failed':
        _completeActivity(
          event,
          label: '分工處理未完成',
          phase: _ActivityPhase.failed,
        );
      case 'actor_supervision':
        _addActivity(event, '分工已由上層調整', detail: _supervisionDetail(event));
      case 'actor_cancelled':
        _completeActivity(
          event,
          label: '分工已取消',
          phase: _ActivityPhase.cancelled,
        );
      case 'actor_resources_linked':
        _addActivity(
          event,
          '分工處理完成',
          phase: _ActivityPhase.completed,
          links: _activityResourceLinks(event),
        );
      case 'tool_call_update':
        final terminal = _ompToolTerminalPhase(event);
        if (terminal == null) {
          _addActivity(event, '正在處理工具工作', detail: _ompToolDetail(event));
        } else {
          _completeActivity(
            event,
            label: terminal == _ActivityPhase.failed ? '工具工作未完成' : '工具工作完成',
            phase: terminal,
          );
        }
      case 'diff':
        _addActivity(event, '正在整理程式變更', detail: _safePathDetail(event));
      case 'artifact':
        final links = _activityResourceLinks(event);
        _addActivity(
          event,
          '已保存執行產物',
          correlationKey:
              links.isEmpty
                  ? _eventSpecificKey('artifact', event)
                  : 'artifact:${links.first.uri}',
          phase: _ActivityPhase.completed,
          links: links,
        );
      case 'memory_recall':
        _addActivity(
          event,
          '已載入相關記憶',
          detail: '僅使用本次對話授權範圍',
          correlationKey: _eventSpecificKey('memory', event),
          phase: _ActivityPhase.completed,
          links: _activityResourceLinks(event),
        );
      case 'dream_queued':
        _addActivity(event, '記憶整理已排程', phase: _ActivityPhase.paused);
      case 'dream_started':
        _addActivity(event, '正在整理記憶');
      case 'dream_progress':
        _addActivity(event, '正在整理記憶', detail: _dreamPhaseLabel(event));
      case 'dream_completed':
        _completeActivity(event, label: '記憶整理完成');
      case 'dream_failed':
        _completeActivity(
          event,
          label: '記憶整理未完成',
          phase: _ActivityPhase.failed,
        );
      case 'cron_run_started':
        _addActivity(event, '排程工作正在執行');
      case 'cron_run_completed':
        _completeActivity(event, label: '排程工作完成');
      case 'drive_put':
      case 'drive_transduced':
      case 'drive_path_proposed':
      case 'drive_write_proposed':
      case 'drive_filed':
      case 'drive_moved':
      case 'drive_tagged':
      case 'project_linked':
      case 'project_unlinked':
        _addActivity(
          event,
          _driveEventLabel(event.type),
          detail: _driveEventDetail(event),
          phase: _ActivityPhase.completed,
        );
      case 'drive_organizer_started':
        _addActivity(event, '正在整理 Drive');
      case 'drive_organizer_completed':
        _completeActivity(event, label: 'Drive 整理完成');
      case 'drive_organizer_failed':
        _completeActivity(
          event,
          label: 'Drive 整理未完成',
          phase: _ActivityPhase.failed,
        );
      case 'egress_started':
        _addActivity(event, '正在使用受限外部連線', detail: _egressDetail(event));
      case 'egress_completed':
        _completeActivity(event, label: '受限外部連線完成');
      case 'egress_failed':
        _completeActivity(
          event,
          label: '受限外部連線未完成',
          phase: _ActivityPhase.failed,
        );
      case 'egress_denied':
        _completeActivity(
          event,
          label: '外部連線已被政策拒絕',
          phase: _ActivityPhase.cancelled,
        );
      case 'secret_handle_issued':
        _addActivity(
          event,
          '已取得目的地限定憑證',
          detail: '憑證內容不會顯示或交給模型',
          phase: _ActivityPhase.completed,
        );
      default:
        break;
    }
  }

  void _upsertProposal(MikuEvent event) {
    final proposalId = _firstEventString(event.data, const [
      'proposalId',
      'proposal_id',
      'targetProposalId',
    ]);
    if (proposalId.isEmpty) return;
    final kind = _firstEventString(event.data, const ['kind', 'action']);
    final status =
        _eventStatus(event).isEmpty ? 'pending' : _eventStatus(event);
    for (final item in _items.whereType<_ProposalItem>()) {
      if (item.proposalId != proposalId) continue;
      item
        ..kind = kind.isEmpty ? item.kind : kind
        ..status = status;
      return;
    }
    _items.add(
      _ProposalItem(
        key: 'proposal-$proposalId',
        proposalId: proposalId,
        kind: kind,
        status: status,
      ),
    );
  }

  Future<void> _openEventResource(String uri) async {
    final session = _session;
    if (session == null || !_isPreviewableResourceUri(uri)) return;
    final preview = widget.client.previewResource(session.id, uri);
    await showModalBottomSheet<void>(
      context: context,
      useSafeArea: true,
      isScrollControlled: true,
      showDragHandle: true,
      builder:
          (context) => _EventResourcePreviewSheet(uri: uri, preview: preview),
    );
  }

  void _recordActorStatus(MikuEvent event) {
    final status = _eventStatus(event);
    const terminal = {'completed', 'failed', 'cancelled', 'terminated'};
    _addActivity(
      event,
      switch (status) {
        'blocked' || 'waiting' => '分工代理正在等待',
        'failed' => '分工處理未完成',
        'cancelled' || 'terminated' => '分工已停止',
        'completed' => '分工處理完成',
        _ => '分工代理正在處理',
      },
      phase:
          status == 'failed'
              ? _ActivityPhase.failed
              : status == 'cancelled' || status == 'terminated'
              ? _ActivityPhase.cancelled
              : status == 'blocked' || status == 'waiting'
              ? _ActivityPhase.paused
              : terminal.contains(status)
              ? _ActivityPhase.completed
              : _ActivityPhase.running,
    );
  }
}

class _ActivityStatusMark extends StatelessWidget {
  const _ActivityStatusMark({required this.activity});

  final _ActivityItem activity;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    if (activity.phase == _ActivityPhase.running) {
      return CircularProgressIndicator(strokeWidth: 1.6, color: palette.miku);
    }
    final colors = Theme.of(context).colorScheme;
    final (icon, color) = switch (activity.phase) {
      _ActivityPhase.paused => (Icons.hourglass_top_rounded, palette.warm),
      _ActivityPhase.failed => (Icons.error_outline_rounded, colors.error),
      _ActivityPhase.cancelled => (Icons.block_rounded, palette.muted),
      _ActivityPhase.completed => (Icons.check_rounded, palette.miku),
      _ActivityPhase.running => (Icons.more_horiz_rounded, palette.miku),
    };
    return Icon(icon, size: 13, color: color);
  }
}

class _ProposalRow extends StatelessWidget {
  const _ProposalRow({required this.proposal});

  final _ProposalItem proposal;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final pending = proposal.status == 'pending';
    final failed = const {
      'denied',
      'failed',
      'cancelled',
      'timed_out',
    }.contains(proposal.status);
    return Semantics(
      key: Key('proposal-${proposal.proposalId}'),
      liveRegion: true,
      label:
          '${_proposalKindLabel(proposal.kind)}，${_proposalStatusLabel(proposal.status)}',
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox.square(
            dimension: 20,
            child:
                pending
                    ? CircularProgressIndicator(
                      strokeWidth: 1.6,
                      color: palette.warm,
                    )
                    : Icon(
                      failed ? Icons.block_rounded : Icons.task_alt_rounded,
                      size: 17,
                      color: failed ? palette.muted : palette.miku,
                    ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  _proposalStatusLabel(proposal.status),
                  style: Theme.of(context).textTheme.bodySmall?.copyWith(
                    color: palette.muted,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                Text(
                  _proposalKindLabel(proposal.kind),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall?.copyWith(
                    color: palette.muted.withValues(alpha: 0.78),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _EventResourcePreviewSheet extends StatelessWidget {
  const _EventResourcePreviewSheet({required this.uri, required this.preview});

  final String uri;
  final Future<ResourcePreview> preview;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    return FractionallySizedBox(
      key: const Key('event-resource-preview'),
      heightFactor: 0.72,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    '唯讀資源預覽',
                    style: Theme.of(context).textTheme.titleLarge?.copyWith(
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                IconButton(
                  tooltip: '關閉資源預覽',
                  onPressed: () => Navigator.of(context).pop(),
                  icon: const Icon(Icons.close_rounded),
                ),
              ],
            ),
            SelectableText(
              _boundedEventText(uri, maxCharacters: 512) ?? uri,
              maxLines: 2,
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                color: palette.muted,
                fontFamily: 'monospace',
              ),
            ),
            const SizedBox(height: 14),
            Expanded(
              child: FutureBuilder<ResourcePreview>(
                future: preview,
                builder: (context, snapshot) {
                  if (snapshot.connectionState != ConnectionState.done) {
                    return const Center(child: CircularProgressIndicator());
                  }
                  final value = snapshot.data;
                  if (value == null) {
                    return const Center(child: Text('這個資源目前無法預覽。'));
                  }
                  final source =
                      value.content.trim().isNotEmpty
                          ? value.content
                          : value.preview;
                  final bounded =
                      _boundedEventText(source, maxCharacters: 6000) ??
                      '這個資源沒有可顯示的文字預覽。';
                  return SingleChildScrollView(
                    child: SelectableText(
                      bounded,
                      key: const Key('event-resource-preview-content'),
                    ),
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _RollbackReviewDetails {
  const _RollbackReviewDetails({
    required this.kind,
    required this.targetId,
    required this.expectedActiveDigest,
    required this.targetDigest,
  });

  final String kind;
  final String targetId;
  final String expectedActiveDigest;
  final String? targetDigest;
}

class _RollbackProposalDetails extends StatelessWidget {
  const _RollbackProposalDetails({required this.details});

  final _RollbackReviewDetails details;

  @override
  Widget build(BuildContext context) {
    final palette = _Palette.of(context);
    final kindLabel = switch (details.kind) {
      'mode_addendum_rollback' => 'Mode guidance',
      'persona_addendum_rollback' => 'Persona guidance',
      _ => 'Skill',
    };
    return Container(
      key: const Key('rollback-proposal-details'),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: palette.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            '$kindLabel · ${_boundedEventText(details.targetId, maxCharacters: 120)}',
            style: Theme.of(context).textTheme.labelLarge,
          ),
          const SizedBox(height: 8),
          _RollbackDigestLine(
            label: '目前啟用',
            value: details.expectedActiveDigest,
          ),
          const SizedBox(height: 6),
          _RollbackDigestLine(
            label: '切換目標',
            value: details.targetDigest ?? 'base（停用 addendum）',
          ),
          const SizedBox(height: 8),
          Text(
            '只有目前 digest 仍相符時才會執行。',
            style: Theme.of(
              context,
            ).textTheme.bodySmall?.copyWith(color: palette.muted),
          ),
        ],
      ),
    );
  }
}

class _RollbackDigestLine extends StatelessWidget {
  const _RollbackDigestLine({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: Theme.of(context).textTheme.labelSmall),
        SelectableText(
          _boundedEventText(value, maxCharacters: 160) ?? '—',
          maxLines: 2,
          style: Theme.of(
            context,
          ).textTheme.bodySmall?.copyWith(fontFamily: 'monospace'),
        ),
      ],
    );
  }
}

_RollbackReviewDetails? _rollbackReviewDetails(Map<String, Object?> scope) {
  final kind = _firstEventString(scope, const ['kind']);
  if (!const {
    'mode_addendum_rollback',
    'persona_addendum_rollback',
    'skill_rollback',
  }.contains(kind)) {
    return null;
  }
  final targetId = _firstEventString(scope, const [
    'modeId',
    'personaId',
    'name',
  ]);
  final expected = _firstEventString(scope, const ['expectedActiveDigest']);
  final rawTarget = scope['targetDigest'];
  return _RollbackReviewDetails(
    kind: kind,
    targetId: targetId.isEmpty ? '未命名目標' : targetId,
    expectedActiveDigest: expected.isEmpty ? '未提供' : expected,
    targetDigest:
        rawTarget == null || _string(rawTarget).trim().isEmpty
            ? null
            : _string(rawTarget).trim(),
  );
}
