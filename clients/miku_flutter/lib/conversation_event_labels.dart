part of 'conversation_screen.dart';

// Pure formatting and extraction helpers for conversation event
// labels, details, correlation keys, and terminal phases.

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
