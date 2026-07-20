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

String? _activityCorrelationKey(MikuEvent event) {
  final data = event.data;
  if (event.type.startsWith('cell_')) {
    return _prefixedKey(
      'cell',
      _firstEventString(data, const ['cellId', 'cell_id']),
    );
  }
  if (event.type.startsWith('effect_') || event.type.startsWith('scope_')) {
    return _prefixedKey(
      'node',
      _firstEventString(data, const ['nodeId', 'node_id']),
    );
  }
  if (event.type.startsWith('actor_')) {
    return _prefixedKey(
      'actor',
      _firstEventString(data, const [
        'actorId',
        'actor_id',
        'failedActorId',
        'failed_actor_id',
        'from',
      ]),
    );
  }
  if (const {'tool_call', 'tool_call_update', 'diff'}.contains(event.type)) {
    final nested = _eventMap(data['toolCall'] ?? data['toolCallUpdate']);
    final id = _firstEventString(data, const ['toolCallId', 'tool_call_id']);
    final nestedId = _firstEventString(nested, const [
      'toolCallId',
      'tool_call_id',
    ]);
    return _prefixedKey('omp', nestedId.isEmpty ? id : nestedId);
  }
  if (event.type == 'mcp_invocation') {
    final request = _firstEventString(data, const ['requestDigest']);
    if (request.isEmpty) return null;
    return [
      'mcp',
      _firstEventString(data, const ['server']),
      _firstEventString(data, const ['objectKind']),
      _firstEventString(data, const ['objectName']),
      _firstEventString(data, const ['targetDigest']),
      request,
    ].join(':');
  }
  if (event.type.startsWith('dream_')) {
    return _prefixedKey('dream', _firstEventString(data, const ['dreamId']));
  }
  if (event.type.startsWith('cron_run_')) {
    return _prefixedKey('cron', _firstEventString(data, const ['runId']));
  }
  if (event.type.startsWith('drive_')) {
    return _prefixedKey(
      'drive',
      _firstEventString(data, const [
        'runId',
        'entryId',
        'proposalId',
        'linkedUri',
      ]),
    );
  }
  if (event.type.startsWith('egress_')) {
    return _prefixedKey(
      'egress',
      _firstEventString(data, const ['effectId', 'auditId']),
    );
  }
  if (event.type.startsWith('secret_')) {
    return _prefixedKey('secret', _firstEventString(data, const ['secretId']));
  }
  return null;
}

String? _eventSpecificKey(String prefix, MikuEvent event) {
  final correlated = _activityCorrelationKey(event);
  if (correlated != null) return '$prefix:$correlated';
  if (event.turnId != null && event.turnId!.isNotEmpty) {
    return '$prefix:turn:${event.turnId}';
  }
  if (event.id != null && event.id!.isNotEmpty) {
    return '$prefix:event:${event.id}';
  }
  return null;
}

String? _prefixedKey(String prefix, String value) =>
    value.trim().isEmpty ? null : '$prefix:${value.trim()}';

Map<String, Object?> _eventMap(Object? value) {
  if (value is Map<String, Object?>) return value;
  if (value is Map) {
    return value.map((key, value) => MapEntry(key.toString(), value));
  }
  return const {};
}

String _firstEventString(Map data, List<String> keys) {
  for (final key in keys) {
    final value = data[key];
    if (value == null) continue;
    final text = value.toString().trim();
    if (text.isNotEmpty && text != 'null') return text;
  }
  return '';
}

String? _boundedEventText(Object? value, {int maxCharacters = 240}) {
  if (value == null || value is Map || value is Iterable) return null;
  final text = value.toString().trim();
  if (text.isEmpty) return null;
  final compact = text.replaceAll(RegExp(r'[\t\r ]+'), ' ');
  if (compact.length <= maxCharacters) return compact;
  return '${compact.substring(0, maxCharacters)}…';
}

void _mergeActivityLinks(
  _ActivityItem item,
  List<_ActivityResourceLink> links,
) {
  for (final link in links) {
    if (item.links.any((existing) => existing.uri == link.uri)) continue;
    item.links.add(link);
  }
}

List<_ActivityResourceLink> _activityResourceLinks(MikuEvent event) {
  final candidates = <String>[
    _firstEventString(event.data, const ['artifactUri', 'artifact_uri']),
    _firstEventString(event.data, const ['historyUri', 'history_uri']),
    _firstEventString(event.data, const ['resourceUri', 'summaryUri', 'uri']),
  ];
  final artifact = _eventMap(event.data['artifact']);
  candidates.add(_firstEventString(artifact, const ['uri']));
  final links = <_ActivityResourceLink>[];
  for (final uri in candidates) {
    if (!_isPreviewableResourceUri(uri) ||
        links.any((link) => link.uri == uri)) {
      continue;
    }
    links.add(
      _ActivityResourceLink(
        uri: uri,
        kind: Uri.parse(uri).scheme.toLowerCase(),
      ),
    );
  }
  return links;
}

bool _isPreviewableResourceUri(String uri) {
  if (uri.length > 2048 || !uri.contains('://')) return false;
  final parsed = Uri.tryParse(uri);
  if (parsed == null || parsed.scheme.isEmpty) return false;
  return !const {
    'http',
    'https',
    'file',
    'data',
  }.contains(parsed.scheme.toLowerCase());
}

String _terminalActivityLabel(String type, _ActivityPhase phase) =>
    switch (phase) {
      _ActivityPhase.failed => '工作未完成',
      _ActivityPhase.cancelled => '工作已取消',
      _ => '工作完成',
    };

_ActivityPhase _runtimeTerminalPhase(MikuEvent event) {
  final status = _eventStatus(event);
  if (status.isEmpty ||
      const {
        'completed',
        'done',
        'succeeded',
        'success',
        'ok',
      }.contains(status)) {
    return _ActivityPhase.completed;
  }
  if (const {'cancelled', 'canceled', 'denied'}.contains(status)) {
    return _ActivityPhase.cancelled;
  }
  return _ActivityPhase.failed;
}

String _runtimeTerminalLabel(String type, _ActivityPhase phase) {
  final subject = type.startsWith('cell_') ? '安全工作環境' : '受控能力';
  return switch (phase) {
    _ActivityPhase.completed => '$subject執行完成',
    _ActivityPhase.cancelled => '$subject執行已取消',
    _ => '$subject執行未完成',
  };
}

_ActivityPhase _mcpInvocationTerminalPhase(String status) {
  if (const {
    'completed',
    'done',
    'succeeded',
    'success',
    'ok',
  }.contains(status)) {
    return _ActivityPhase.completed;
  }
  if (const {'denied', 'cancelled', 'canceled'}.contains(status)) {
    return _ActivityPhase.cancelled;
  }
  return _ActivityPhase.failed;
}

String _mcpInvocationTerminalLabel(_ActivityPhase phase) => switch (phase) {
  _ActivityPhase.completed => '外部資源查詢完成',
  _ActivityPhase.cancelled => '外部資源查詢已拒絕或取消',
  _ => '外部資源查詢未完成',
};

String _eventStatus(MikuEvent event) {
  final direct = _firstEventString(event.data, const ['status']);
  if (direct.isNotEmpty) return direct.toLowerCase();
  final tool = _eventMap(
    event.data['toolCall'] ?? event.data['toolCallUpdate'],
  );
  final fields = _eventMap(tool['fields']);
  return _firstEventString(fields, const ['status']).toLowerCase();
}

String? _displayDetail(MikuEvent event) {
  final value = event.data['value'];
  if (value is String || value is num || value is bool) {
    return _boundedEventText(value);
  }
  final spec = event.data['spec'];
  if (spec is String && spec == '[redacted]') return '[redacted]';
  return '有界結果已就緒';
}

String? _scopeProgress(MikuEvent event) {
  final completed = _firstEventString(event.data, const ['completed']);
  final total = _firstEventString(event.data, const ['total']);
  if (completed.isEmpty && total.isEmpty) return null;
  return '${completed.isEmpty ? '0' : completed} / ${total.isEmpty ? '？' : total}';
}

String? _supervisionDetail(MikuEvent event) => _boundedEventText(
  _firstEventString(event.data, const ['decision']),
  maxCharacters: 80,
);

_ActivityPhase? _ompToolTerminalPhase(MikuEvent event) {
  final status = _eventStatus(event);
  if (const {'completed', 'done', 'succeeded'}.contains(status)) {
    return _ActivityPhase.completed;
  }
  if (const {'failed', 'error'}.contains(status)) return _ActivityPhase.failed;
  if (const {'cancelled', 'canceled'}.contains(status)) {
    return _ActivityPhase.cancelled;
  }
  return null;
}

String? _ompToolDetail(MikuEvent event) {
  final tool = _eventMap(
    event.data['toolCall'] ?? event.data['toolCallUpdate'],
  );
  final fields = _eventMap(tool['fields']);
  return _boundedEventText(
    _firstEventString(tool, const ['title', 'name']).isNotEmpty
        ? _firstEventString(tool, const ['title', 'name'])
        : _firstEventString(fields, const ['title', 'name']),
    maxCharacters: 120,
  );
}

String? _safePathDetail(MikuEvent event) => _boundedEventText(
  _firstEventString(event.data, const ['path']),
  maxCharacters: 180,
);

String? _dreamPhaseLabel(MikuEvent event) => switch (_firstEventString(
  event.data,
  const ['phase'],
)) {
  'input_collected' => '已收集有界輸入',
  'summary_written' => '已更新摘要',
  'reflection_written' => '已更新反思摘要',
  'summary_rollup_updated' => '已更新長期摘要',
  _ => null,
};

String _driveEventLabel(String type) => switch (type) {
  'drive_put' => 'Drive 文件已保存',
  'drive_transduced' => 'Drive 文件已整理',
  'drive_path_proposed' || 'drive_write_proposed' => 'Drive 變更已提出',
  'drive_filed' => 'Drive 文件已歸檔',
  'drive_moved' => 'Drive 文件已移動',
  'drive_tagged' => 'Drive 標籤已更新',
  'project_linked' => 'Project 資料夾已連結',
  'project_unlinked' => 'Project 資料夾已解除連結',
  _ => 'Drive 已更新',
};

String? _driveEventDetail(MikuEvent event) {
  final preview = _eventMap(event.data['preview']);
  return _boundedEventText(
    _firstEventString(preview, const ['title', 'subtitle']),
    maxCharacters: 160,
  );
}

String? _egressDetail(MikuEvent event) {
  final method = _firstEventString(event.data, const ['method']);
  final destination = _firstEventString(event.data, const ['destinationId']);
  if (method.isEmpty && destination.isEmpty) return null;
  return _boundedEventText('$method $destination', maxCharacters: 120);
}

String _proposalStatusLabel(String status) => switch (status) {
  'pending' => '變更提案等待確認',
  'approved' || 'applied' => '變更提案已核准',
  'denied' => '變更提案已拒絕',
  'failed' => '變更提案未完成',
  'cancelled' || 'timed_out' => '變更提案已結束',
  _ => '變更提案已更新',
};

String _proposalKindLabel(String kind) => switch (kind) {
  'memory' || 'profile_fact' || 'recall_chunk' => '記憶變更',
  'persona' ||
  'persona_addendum' ||
  'persona_addendum_rollback' => 'Persona guidance',
  'mode' || 'mode_addendum' || 'mode_addendum_rollback' => 'Mode guidance',
  'skill' || 'skill_rollback' => 'Skill 版本',
  'drive' || 'move' || 'tag' => 'Drive 整理',
  _ => '經審核的變更',
};
