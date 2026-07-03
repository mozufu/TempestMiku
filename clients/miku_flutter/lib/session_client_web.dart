// ignore: avoid_web_libraries_in_flutter
import 'dart:html';
import 'dart:async';
import 'dart:convert';

import 'session_models.dart';

MikuSessionClient createClient() => WebMikuSessionClient();

class WebMikuSessionClient implements MikuSessionClient {
  static const _sessionIdKey = 'tempestmiku.sessionId';
  static const _lastEventIdKey = 'tempestmiku.lastEventId';

  @override
  Future<MikuSession> createOrReuseSession() async {
    final storedId = window.localStorage[_sessionIdKey];
    if (storedId != null && storedId.isNotEmpty) {
      try {
        final json = await _request('GET', '/sessions/$storedId');
        final session = _sessionFromJson(
          json,
          lastEventId: window.localStorage[_lastEventIdKey],
        );
        _rememberSession(session);
        return session;
      } catch (_) {
        window.localStorage.remove(_sessionIdKey);
        window.localStorage.remove(_lastEventIdKey);
      }
    }
    return createSession();
  }

  @override
  Future<MikuSession> createSession() async {
    final json = await _request('POST', '/sessions');
    final session = _sessionFromJson(json);
    _rememberSession(session);
    return session;
  }

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) {
    final controller = StreamController<MikuEvent>();
    final resumeId = lastEventId ?? window.localStorage[_lastEventIdKey];
    final suffix = resumeId == null || resumeId.isEmpty
        ? ''
        : '?lastEventId=${Uri.encodeQueryComponent(resumeId)}';
    final source = EventSource('/sessions/$sessionId/events$suffix');
    for (final type in [
      'text',
      'final',
      'mode',
      'approval',
      'approval_resolved',
      'diff',
      'artifact',
      'tool_call',
      'tool_call_update',
      'cell_start',
      'cell_result',
      'write_proposal',
      'error',
    ]) {
      source.addEventListener(type, (Event event) {
        final message = event as MessageEvent;
        final eventId = message.lastEventId;
        final data =
            (jsonDecode(message.data as String) as Map).cast<String, Object?>();
        if (eventId.isNotEmpty && shouldRememberEventId(type, data)) {
          rememberLastEventId(sessionId, eventId);
        }
        controller.add(
          MikuEvent(
            type: type,
            id: eventId,
            data: data,
          ),
        );
      });
    }
    source.onOpen.listen((_) {
      if (!controller.isClosed) {
        controller.add(
          const MikuEvent(
            type: 'connection',
            data: {'status': 'connected'},
          ),
        );
      }
    });
    source.onError.listen((_) {
      if (!controller.isClosed) {
        controller.add(
          const MikuEvent(
            type: 'connection',
            data: {'status': 'reconnecting'},
          ),
        );
      }
    });
    controller.onCancel = source.close;
    return controller.stream;
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    if (window.localStorage[_sessionIdKey] == sessionId) {
      window.localStorage[_lastEventIdKey] = lastEventId;
    }
  }

  @override
  Future<void> sendMessage(String sessionId, String content) async {
    await _request(
      'POST',
      '/sessions/$sessionId/messages',
      body: {'content': content},
    );
  }

  @override
  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  }) async {
    await _request(
      'POST',
      '/sessions/$sessionId/approvals/$approvalId',
      body: {
        'decision': decision,
        if (optionId != null) 'optionId': optionId,
      },
    );
  }

  @override
  Future<void> lockMode(String sessionId, String mode) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/lock',
      body: {'mode': mode, 'reason': 'flutter lock'},
    );
  }

  @override
  Future<void> unlockMode(String sessionId) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/unlock',
      body: {'reason': 'flutter unlock'},
    );
  }

  @override
  Future<void> overrideMode(String sessionId, String mode) async {
    await _request(
      'POST',
      '/sessions/$sessionId/mode/override',
      body: {'mode': mode, 'reason': 'flutter override'},
    );
  }

  @override
  Future<ProjectOverview> projectOverview(String sessionId) async {
    final json = await _request('GET', '/sessions/$sessionId/project');
    return ProjectOverview(
      status: json['status'] as String? ?? '',
      nextActions: ((json['nextActions'] as List?) ?? const [])
          .whereType<Map>()
          .map((item) => item['text'] as String? ?? '')
          .where((text) => text.isNotEmpty)
          .toList(),
    );
  }

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) async {
    final query = Uri(queryParameters: {'uri': uri}).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/resources/preview?$query',
    );
    return ResourcePreview(
      uri: json['uri'] as String? ?? uri,
      kind: json['kind'] as String? ?? '',
      mime: json['mime'] as String? ?? '',
      title: json['title'] as String?,
      sizeBytes: json['size_bytes'] as int? ?? json['sizeBytes'] as int? ?? 0,
      preview: json['preview'] as String? ?? '',
      hasMore: json['has_more'] as bool? ?? json['hasMore'] as bool? ?? false,
    );
  }

  @override
  Future<ProjectPromotion> promoteSession(
    String sessionId, {
    String? summary,
    List<String> openLoops = const [],
    List<String> decisions = const [],
    List<String> resources = const [],
  }) async {
    final json = await _request(
      'POST',
      '/sessions/$sessionId/promote',
      body: {
        if (summary != null && summary.trim().isNotEmpty)
          'summary': summary.trim(),
        'openLoops': openLoops,
        'decisions': decisions,
        'resources': resources,
      },
    );
    return ProjectPromotion(
      projectUri: json['projectUri'] as String? ?? '',
      promotedCount: ((json['promoted'] as List?) ?? const []).length,
    );
  }

  Future<Map<String, Object?>> _request(
    String method,
    String path, {
    Map<String, Object?>? body,
  }) async {
    final response = await HttpRequest.request(
      path,
      method: method,
      requestHeaders: {
        if (body != null) 'content-type': 'application/json',
      },
      sendData: body == null ? null : jsonEncode(body),
    );
    final status = response.status ?? 0;
    if (status < 200 || status >= 300) {
      throw StateError('request failed: $status ${response.responseText}');
    }
    final text = response.responseText;
    if (text == null || text.isEmpty) return <String, Object?>{};
    return (jsonDecode(text) as Map).cast<String, Object?>();
  }

  MikuSession _sessionFromJson(
    Map<String, Object?> json, {
    String? lastEventId,
  }) {
    final modeState = (json['mode_state'] as Map?)?.cast<String, Object?>() ??
        (json['modeState'] as Map?)?.cast<String, Object?>() ??
        const <String, Object?>{};
    return MikuSession(
      id: json['id'] as String,
      mode: (json['mode'] as String?) ?? (modeState['mode'] as String?) ?? '',
      label: json['label'] as String? ?? '',
      voiceCap:
          (json['voice_cap'] as String?) ?? (json['voiceCap'] as String?) ?? '',
      defaultScope: (json['default_scope'] as String?) ??
          (json['defaultScope'] as String?) ??
          'global',
      activeSkills: ((json['activeSkills'] as List?) ??
              (json['active_skills'] as List?) ??
              const [])
          .map((skill) => skill.toString())
          .toList(),
      locked:
          modeState['lockSource'] != null || modeState['lock_source'] != null,
      lastEventId: lastEventId,
    );
  }

  void _rememberSession(MikuSession session) {
    window.localStorage[_sessionIdKey] = session.id;
    if (session.lastEventId != null && session.lastEventId!.isNotEmpty) {
      window.localStorage[_lastEventIdKey] = session.lastEventId!;
    }
  }
}
