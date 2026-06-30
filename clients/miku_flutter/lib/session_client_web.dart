// ignore: avoid_web_libraries_in_flutter
import 'dart:html';
import 'dart:async';
import 'dart:convert';

import 'session_models.dart';

MikuSessionClient createClient() => WebMikuSessionClient();

class WebMikuSessionClient implements MikuSessionClient {
  @override
  Future<MikuSession> createSession() async {
    final json = await _request('POST', '/sessions');
    return MikuSession(
      id: json['id'] as String,
      mode: json['mode'] as String,
      label: json['label'] as String,
      voiceCap:
          json['voice_cap'] as String? ?? json['voiceCap'] as String? ?? '',
    );
  }

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) {
    final controller = StreamController<MikuEvent>();
    final suffix = lastEventId == null || lastEventId.isEmpty
        ? ''
        : '?lastEventId=${Uri.encodeQueryComponent(lastEventId)}';
    final source = EventSource('/sessions/$sessionId/events$suffix');
    for (final type in [
      'text',
      'final',
      'mode',
      'approval',
      'approval_resolved',
      'diff',
      'artifact',
    ]) {
      source.addEventListener(type, (Event event) {
        final message = event as MessageEvent;
        controller.add(
          MikuEvent(
            type: type,
            id: message.lastEventId,
            data: (jsonDecode(message.data as String) as Map)
                .cast<String, Object?>(),
          ),
        );
      });
    }
    source.onError.listen((_) {
      if (!controller.isClosed) {
        controller.addError(StateError('event stream disconnected'));
      }
    });
    controller.onCancel = source.close;
    return controller.stream;
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
    String decision,
  ) async {
    await _request(
      'POST',
      '/sessions/$sessionId/approvals/$approvalId',
      body: {'decision': decision},
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
}
