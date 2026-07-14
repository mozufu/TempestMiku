part of 'main.dart';

String _eventText(
  Map<String, Object?> data,
  String key, {
  String? camelKey,
  String fallback = '',
}) {
  final value = data[key] ?? (camelKey == null ? null : data[camelKey]);
  final text = value?.toString() ?? '';
  return text.isEmpty ? fallback : text;
}

Map<String, Object?>? _eventMap(Object? value) {
  if (value is Map<String, Object?>) return value;
  if (value is Map) return value.cast<String, Object?>();
  return null;
}

bool _shouldRefreshDriveFeed(MikuEvent e) {
  if (e.type.startsWith('drive_')) return true;
  return e.type == 'write_proposal' && _eventText(e.data, 'kind') == 'drive';
}

_ActivityItem? _driveActivityFromEvent(MikuEvent e) {
  final data = e.data;
  final preview = _eventMap(data['preview']) ?? const <String, Object?>{};
  final title = _eventText(
    preview,
    'title',
    fallback: switch (e.type) {
      'drive_put' => 'Filed drive document',
      'drive_moved' => 'Moved drive document',
      'drive_tagged' => 'Tagged drive document',
      'drive_linked' => 'Linked project folder',
      'drive_unlinked' => 'Unlinked project folder',
      _ => 'Drive updated',
    },
  );
  return _ActivityItem(
    icon: switch (e.type) {
      'drive_linked' => Icons.folder_shared_outlined,
      'drive_unlinked' => Icons.link_off,
      'drive_moved' => Icons.drive_file_move_outlined,
      'drive_tagged' => Icons.label_outline,
      _ => Icons.insert_drive_file_outlined,
    },
    title: title,
    detail: _joinedDetail([
      _eventText(preview, 'subtitle', fallback: _eventText(data, 'path')),
      _eventText(preview, 'snippet'),
    ]),
    state: _ActivityState.done,
    kind: 'drive',
    resourceUris: _resourceUrisFromEvent(data),
  );
}

String _driveOrganizerDetail(Map<String, Object?> data) {
  final count = (data['proposalCount'] ?? data['proposal_count'])?.toString();
  final tier = _eventText(data, 'tier');
  final apply = data['apply'] == true ? 'apply' : 'propose';
  final parts = [
    if (count != null && count.isNotEmpty) '$count proposals',
    if (tier.isNotEmpty) tier,
    apply,
  ];
  return parts.join(' · ');
}

List<String> _resourceUrisFromEvent(Map<String, Object?> data) {
  final refs =
      ((data['resourceRefs'] ?? data['resource_refs']) as List?) ?? const [];
  final uris = <String>[];

  void add(Object? value) {
    final uri = _cleanResourceUri(value?.toString() ?? '');
    if (_isOpenableResourceUri(uri) && !uris.contains(uri)) uris.add(uri);
  }

  for (final ref in refs.whereType<Map>()) {
    add(_eventText(ref.cast<String, Object?>(), 'uri'));
  }
  for (final key in const [
    'uri',
    'sourceUri',
    'source_uri',
    'proposedUri',
    'proposed_uri',
    'fromUri',
    'from_uri',
    'toUri',
    'to_uri',
    'linkedUri',
    'linked_uri',
  ]) {
    add(data[key]);
  }
  return uris;
}

String _cleanResourceUri(String uri) {
  return uri.trim().replaceAll(RegExp(r'''[.。,"'};:]+$'''), '');
}

bool _isOpenableResourceUri(String uri) {
  return RegExp(
    r'^(?:artifact|workspace|linked|project|drive|memory|agent|history)://',
  ).hasMatch(uri);
}

String _joinedDetail(List<String> parts) {
  return parts
      .map((part) => part.trim())
      .where((part) => part.isNotEmpty)
      .join('\n');
}

List<_AgentStatus> _agentStatuses(List<_ActivityItem> activities) {
  final ordered = <String>[];
  final roles = <String, String>{};
  final states = <String, _ActivityState>{};
  final details = <String, String>{};

  for (final item in activities) {
    final id = item.actorId;
    if (item.kind != 'actor' || id == null || id.isEmpty) continue;
    if (!ordered.contains(id)) ordered.add(id);
    final role = item.role;
    if (role != null && role.isNotEmpty) {
      roles[id] = role;
    }
    states[id] = item.state;
    if (item.detail.trim().isNotEmpty) {
      details[id] = item.detail.trim();
    } else {
      details[id] = item.title;
    }
  }

  return ordered
      .map(
        (id) => _AgentStatus(
          id: id,
          role: roles[id] ?? _roleFromActorId(id),
          state: states[id] ?? _ActivityState.info,
          detail: details[id] ?? '',
        ),
      )
      .toList();
}

String _roleFromActorId(String id) {
  final match = RegExp(r'^([A-Za-z_ -]+)').firstMatch(id);
  final role = match?.group(1)?.trim() ?? '';
  return role.isEmpty ? 'agent' : role.toLowerCase();
}
