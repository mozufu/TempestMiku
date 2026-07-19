part of 'main.dart';

_ActivityItem? _activityFromEvent(MikuEvent e) {
  final data = e.data;
  switch (e.type) {
    case 'tool_call':
      final name = _eventText(data, 'name', fallback: 'execute');
      return _ActivityItem(
        icon: Icons.build_outlined,
        title: '呼叫工具 $name',
        detail: _eventText(data, 'arguments'),
        state: _ActivityState.running,
        monospace: true,
        kind: 'tool',
      );
    case 'tool_call_update':
      final name = _eventText(data, 'name', fallback: 'execute');
      return _ActivityItem(
        icon: Icons.more_horiz,
        title: '更新工具參數 $name',
        detail: _eventText(data, 'arguments'),
        state: _ActivityState.running,
        monospace: true,
        kind: 'tool',
      );
    case 'cell_start':
      return _ActivityItem(
        icon: Icons.terminal,
        title: '執行程式',
        detail: _eventText(data, 'sourcePreview', fallback: '[redacted]'),
        state: _ActivityState.running,
        monospace: true,
        kind: 'cell',
      );
    case 'cell_result':
      final status = _eventText(data, 'status', fallback: 'completed');
      final error = _eventText(data, 'error');
      final resultPreview = _eventText(data, 'resultPreview');
      final failed =
          error.isNotEmpty ||
          const {'failed', 'cancelled', 'timed_out'}.contains(status);
      final preview = error.isNotEmpty ? error : resultPreview;
      final detail = preview.isEmpty ? '[redacted]' : preview;
      return _ActivityItem(
        icon: failed ? Icons.error_outline : Icons.check_circle_outline,
        title: failed ? '程式失敗' : '程式結果',
        detail: detail,
        state: failed ? _ActivityState.failed : _ActivityState.done,
        monospace: true,
        kind: 'cell',
        resourceUris: _extractResources(detail),
      );
    case 'actor_spawned':
      final actorId = _eventText(
        data,
        'actor_id',
        camelKey: 'actorId',
        fallback: _eventText(data, 'id'),
      );
      final role = _eventText(data, 'role', fallback: 'worker');
      return _ActivityItem(
        icon: Icons.account_tree_outlined,
        title: '啟動 $role · $actorId',
        detail: _eventText(data, 'task'),
        state: _ActivityState.running,
        kind: 'actor',
        actorId: actorId,
        role: role,
      );
    case 'actor_status':
      final actorId = _eventText(
        data,
        'actor_id',
        camelKey: 'actorId',
        fallback: _eventText(data, 'id'),
      );
      final status = _eventText(data, 'status', fallback: 'updated');
      return _ActivityItem(
        icon: Icons.timeline,
        title: '$actorId 狀態 $status',
        detail: '',
        state:
            status == 'terminated'
                ? _ActivityState.done
                : _ActivityState.running,
        kind: 'actor',
        actorId: actorId,
      );
    case 'actor_message':
      final actorId = _eventText(
        data,
        'actor_id',
        camelKey: 'actorId',
        fallback: _eventText(data, 'from'),
      );
      return _ActivityItem(
        icon: Icons.chat_bubble_outline,
        title: '$actorId 訊息',
        detail: _eventText(data, 'text', fallback: _eventText(data, 'message')),
        state: _ActivityState.info,
        kind: 'actor',
        actorId: actorId,
      );
    case 'actor_completed':
      final actorId = _eventText(
        data,
        'actor_id',
        camelKey: 'actorId',
        fallback: _eventText(data, 'id'),
      );
      final summary = _eventText(data, 'summary');
      final resources =
          [
            _eventText(data, 'artifact_uri', camelKey: 'artifactUri'),
            _eventText(data, 'history_uri', camelKey: 'historyUri'),
          ].where((uri) => uri.isNotEmpty).toList();
      return _ActivityItem(
        icon: Icons.task_alt,
        title: '完成 $actorId',
        detail: summary,
        state: _ActivityState.done,
        kind: 'actor',
        actorId: actorId,
        resourceUris: resources,
      );
    case 'actor_failed':
      final actorId = _eventText(
        data,
        'actor_id',
        camelKey: 'actorId',
        fallback: _eventText(data, 'id'),
      );
      return _ActivityItem(
        icon: Icons.error_outline,
        title: '$actorId 失敗',
        detail: _eventText(
          data,
          'error',
          fallback: _eventText(
            data,
            'failure_reason',
            camelKey: 'failureReason',
          ),
        ),
        state: _ActivityState.failed,
        kind: 'actor',
        actorId: actorId,
      );
    case 'actor_cancelled':
      final actorId = _eventText(
        data,
        'actor_id',
        camelKey: 'actorId',
        fallback: _eventText(data, 'id'),
      );
      return _ActivityItem(
        icon: Icons.cancel_outlined,
        title: '取消 $actorId',
        detail: _eventText(data, 'reason'),
        state: _ActivityState.failed,
        kind: 'actor',
        actorId: actorId,
      );
    case 'write_proposal':
      final review = EvolutionReviewProposal.fromEvent(data);
      if (review != null) {
        return _ActivityItem(
          icon: Icons.fact_check_outlined,
          title:
              '${review.targetKind} addendum · ${review.targetId} · ${review.status}',
          detail: _joinedDetail([
            review.preview,
            if (review.isAutoCandidate)
              'Auto proposal · ${review.candidateTrigger.replaceAll('_', ' ')} · ${review.evidenceCount ?? 0} evidence records',
            review.applyEnabled
                ? 'Apply enabled'
                : 'Review only · apply disabled',
          ]),
          state: switch (review.status) {
            'approved' => _ActivityState.done,
            'denied' || 'timed_out' || 'cancelled' => _ActivityState.failed,
            _ => _ActivityState.info,
          },
          kind: 'evolution_review',
          resourceUris:
              review.resourceUri.isEmpty ? const [] : [review.resourceUri],
        );
      }
      if (_eventText(data, 'kind') != 'drive') return null;
      final preview = _eventMap(data['preview']);
      return _ActivityItem(
        icon: Icons.rule_folder_outlined,
        title: _eventText(
          preview ?? const <String, Object?>{},
          'title',
          fallback: 'Drive organizer proposal',
        ),
        detail: _joinedDetail([
          _eventText(preview ?? const <String, Object?>{}, 'subtitle'),
          _eventText(preview ?? const <String, Object?>{}, 'snippet'),
        ]),
        state: _ActivityState.info,
        kind: 'drive',
        resourceUris: _resourceUrisFromEvent(data),
      );
    case 'drive_put':
    case 'drive_moved':
    case 'drive_tagged':
    case 'drive_linked':
    case 'drive_unlinked':
      return _driveActivityFromEvent(e);
    case 'drive_organizer_started':
      final tier = _eventText(data, 'tier', fallback: 'conservative');
      final apply = data['apply'] == true ? 'apply' : 'propose';
      return _ActivityItem(
        icon: Icons.rule_folder_outlined,
        title: 'Drive organizer started',
        detail: '$tier · $apply',
        state: _ActivityState.running,
        kind: 'drive',
      );
    case 'drive_organizer_completed':
      return _ActivityItem(
        icon: Icons.task_alt,
        title: 'Drive organizer completed',
        detail: _driveOrganizerDetail(data),
        state: _ActivityState.done,
        kind: 'drive',
        resourceUris: _resourceUrisFromEvent(data),
      );
    case 'drive_organizer_failed':
      return _ActivityItem(
        icon: Icons.error_outline,
        title: 'Drive organizer failed',
        detail: _eventText(
          data,
          'error',
          fallback: _driveOrganizerDetail(data),
        ),
        state: _ActivityState.failed,
        kind: 'drive',
        resourceUris: _resourceUrisFromEvent(data),
      );
  }
  return null;
}

List<String> _extractResources(String text) {
  return RegExp(
        r'''\b(?:artifact|workspace|linked|project|drive)://[^\s),\]\}"']+''',
      )
      .allMatches(text)
      .map((m) => _normalizeResourceUri(m.group(0)!))
      .toSet()
      .toList();
}

String _normalizeResourceUri(String uri) {
  return _cleanResourceUri(uri);
}
