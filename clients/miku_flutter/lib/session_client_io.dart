import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:shared_preferences/shared_preferences.dart';

import 'session_models.dart';

MikuSessionClient createClient() => NativeMikuSessionClient();

class NativeMikuSessionClient implements MikuSessionClient, ServerTargetClient {
  NativeMikuSessionClient();

  static const _serverBaseUrlKey = 'tempestmiku.serverBaseUrl';
  static const _sessionIdKey = 'tempestmiku.sessionId';
  static const _lastEventIdKey = 'tempestmiku.lastEventId';
  static const _configuredServerBaseUrl =
      String.fromEnvironment('MIKU_SERVER_URL');
  static const _defaultServerBaseUrl = 'http://10.0.2.2:3000';

  final HttpClient _http = HttpClient();

  @override
  Future<String> serverBaseUrl() async {
    final prefs = await SharedPreferences.getInstance();
    final stored = prefs.getString(_serverBaseUrlKey);
    if (stored != null && stored.trim().isNotEmpty) {
      return _normalizeServerBaseUrl(stored);
    }
    if (_configuredServerBaseUrl.trim().isNotEmpty) {
      return _normalizeServerBaseUrl(_configuredServerBaseUrl);
    }
    return _defaultServerBaseUrl;
  }

  @override
  Future<void> setServerBaseUrl(String baseUrl) async {
    final normalized = _normalizeServerBaseUrl(baseUrl);
    final prefs = await SharedPreferences.getInstance();
    final previous = prefs.getString(_serverBaseUrlKey);
    await prefs.setString(_serverBaseUrlKey, normalized);
    if (previous != normalized) {
      await prefs.remove(_sessionIdKey);
      await prefs.remove(_lastEventIdKey);
    }
  }

  @override
  Future<ModeCatalog> modeCatalog() async {
    final json = await _request('GET', '/modes');
    return ModeCatalog.fromJson(json);
  }

  @override
  Future<MikuSession> createOrReuseSession() async {
    final prefs = await SharedPreferences.getInstance();
    final storedId = prefs.getString(_sessionIdKey);
    if (storedId != null && storedId.isNotEmpty) {
      try {
        final json = await _request('GET', '/sessions/$storedId');
        final session = _sessionFromJson(
          json,
          lastEventId: prefs.getString(_lastEventIdKey),
        );
        await _rememberSession(session);
        return session;
      } catch (_) {
        await prefs.remove(_sessionIdKey);
        await prefs.remove(_lastEventIdKey);
      }
    }
    return createSession();
  }

  @override
  Future<MikuSession> createSession() async {
    final json = await _request('POST', '/sessions');
    final session = _sessionFromJson(json);
    await _rememberSession(session);
    return session;
  }

  @override
  Future<List<SessionSummary>> listSessions({int limit = 30}) async {
    final query = Uri(queryParameters: {'limit': '$limit'}).query;
    final json = await _request('GET', '/sessions?$query');
    return ((json['sessions'] as List?) ?? const [])
        .whereType<Map>()
        .map((item) => _summaryFromJson(item.cast<String, Object?>()))
        .toList();
  }

  @override
  Future<LoadedSession> loadSession(String sessionId) async {
    final json = await _request('GET', '/sessions/$sessionId/messages');
    final lastEventId = _nullableString(
      json['lastEventId'] ?? json['last_event_id'],
    );
    final session = _sessionFromJson(json, lastEventId: lastEventId);
    await _rememberSession(session);
    final messages = ((json['messages'] as List?) ?? const [])
        .whereType<Map>()
        .map((item) => _messageFromJson(item.cast<String, Object?>()))
        .toList();
    final pendingEvents = ((json['pendingEvents'] as List?) ??
            (json['pending_events'] as List?) ??
            const [])
        .whereType<Map>()
        .map((item) => _eventFromJson(item.cast<String, Object?>()))
        .toList();
    return LoadedSession(
      session: session,
      messages: messages,
      pendingEvents: pendingEvents,
    );
  }

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) {
    final controller = StreamController<MikuEvent>();
    final eventClient = HttpClient();
    var closed = false;
    controller.onCancel = () {
      closed = true;
      eventClient.close(force: true);
    };
    unawaited(_pumpEvents(
      controller,
      eventClient,
      () => closed || controller.isClosed,
      sessionId,
      lastEventId,
    ));
    return controller.stream;
  }

  Future<void> _pumpEvents(
    StreamController<MikuEvent> controller,
    HttpClient eventClient,
    bool Function() isClosed,
    String sessionId,
    String? initialLastEventId,
  ) async {
    var resumeId = initialLastEventId ?? await _storedLastEventId();
    while (!isClosed()) {
      try {
        final request = await eventClient.getUrl(
          await _resolve(_eventsPath(sessionId, resumeId)),
        );
        request.headers
          ..set(HttpHeaders.acceptHeader, 'text/event-stream')
          ..set(HttpHeaders.cacheControlHeader, 'no-cache');
        if (resumeId != null && resumeId.isNotEmpty) {
          request.headers.set('Last-Event-ID', resumeId);
        }
        final response = await request.close();
        if (response.statusCode < 200 || response.statusCode >= 300) {
          throw StateError('event stream failed: ${response.statusCode}');
        }
        if (!isClosed()) {
          controller.add(
            const MikuEvent(
              type: 'connection',
              data: {'status': 'connected'},
            ),
          );
        }
        await for (final event in _decodeSse(response)) {
          if (isClosed()) break;
          final eventId = event.id;
          if (eventId != null &&
              eventId.isNotEmpty &&
              shouldRememberEventId(event.type, event.data)) {
            resumeId = eventId;
            await _rememberLastEventId(sessionId, eventId);
          }
          controller.add(event);
        }
      } catch (_) {
        if (!isClosed()) {
          controller.add(
            const MikuEvent(
              type: 'connection',
              data: {'status': 'reconnecting'},
            ),
          );
          await Future<void>.delayed(const Duration(seconds: 2));
        }
      }
    }
    eventClient.close(force: true);
  }

  Stream<MikuEvent> _decodeSse(HttpClientResponse response) async* {
    var type = 'message';
    String? id;
    final dataLines = <String>[];

    MikuEvent? flush() {
      if (dataLines.isEmpty) {
        type = 'message';
        id = null;
        return null;
      }
      final dataText = dataLines.join('\n');
      Map<String, Object?> data;
      try {
        final decoded = jsonDecode(dataText);
        data = decoded is Map
            ? decoded.cast<String, Object?>()
            : {'value': decoded};
      } catch (_) {
        data = {'text': dataText};
      }
      final event = MikuEvent(type: type, id: id, data: data);
      type = 'message';
      id = null;
      dataLines.clear();
      return event;
    }

    await for (final line
        in response.transform(utf8.decoder).transform(const LineSplitter())) {
      if (line.isEmpty) {
        final event = flush();
        if (event != null) yield event;
        continue;
      }
      if (line.startsWith(':')) continue;
      final colon = line.indexOf(':');
      final field = colon == -1 ? line : line.substring(0, colon);
      var value = colon == -1 ? '' : line.substring(colon + 1);
      if (value.startsWith(' ')) value = value.substring(1);
      switch (field) {
        case 'event':
          type = value.isEmpty ? 'message' : value;
        case 'data':
          dataLines.add(value);
        case 'id':
          id = value;
      }
    }
    final event = flush();
    if (event != null) yield event;
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    unawaited(_rememberLastEventId(sessionId, lastEventId));
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
  Future<DriveFeed> driveFeed(
    String sessionId, {
    int limit = 20,
    String? project,
  }) async {
    final trimmedProject = project?.trim();
    final query = Uri(
      queryParameters: {
        'limit': '$limit',
        if (trimmedProject != null && trimmedProject.isNotEmpty)
          'project': trimmedProject,
      },
    ).query;
    final json =
        await _request('GET', '/sessions/$sessionId/drive/feed?$query');
    return DriveFeed.fromJson(json);
  }

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) async {
    final query = Uri(queryParameters: {'uri': uri}).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/resources/preview?$query',
    );
    return _resourcePreviewFromJson(json, uri);
  }

  @override
  Future<ResourcePreview> resolveResource(String sessionId, String uri) async {
    final query = Uri(queryParameters: {'uri': uri}).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/resources/resolve?$query',
    );
    return _resourcePreviewFromJson(json, uri);
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
    final request = await _http.openUrl(method, await _resolve(path));
    request.headers.set(HttpHeaders.acceptHeader, 'application/json');
    if (body != null) {
      request.headers.contentType = ContentType.json;
      request.write(jsonEncode(body));
    }
    final response = await request.close();
    final text = await response.transform(utf8.decoder).join();
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw StateError('request failed: ${response.statusCode} $text');
    }
    if (text.isEmpty) return <String, Object?>{};
    return (jsonDecode(text) as Map).cast<String, Object?>();
  }

  Future<Uri> _resolve(String path) async {
    final base = await serverBaseUrl();
    final baseUri = Uri.parse(base.endsWith('/') ? base : '$base/');
    final relative = path.startsWith('/') ? path.substring(1) : path;
    return baseUri.resolve(relative);
  }

  String _eventsPath(String sessionId, String? lastEventId) {
    if (lastEventId == null || lastEventId.isEmpty) {
      return '/sessions/$sessionId/events';
    }
    final query = Uri(queryParameters: {'lastEventId': lastEventId}).query;
    return '/sessions/$sessionId/events?$query';
  }

  Future<String?> _storedLastEventId() async {
    final prefs = await SharedPreferences.getInstance();
    return prefs.getString(_lastEventIdKey);
  }

  Future<void> _rememberSession(MikuSession session) async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_sessionIdKey, session.id);
    if (session.lastEventId != null && session.lastEventId!.isNotEmpty) {
      await prefs.setString(_lastEventIdKey, session.lastEventId!);
    } else {
      await prefs.remove(_lastEventIdKey);
    }
  }

  Future<void> _rememberLastEventId(
    String sessionId,
    String lastEventId,
  ) async {
    final prefs = await SharedPreferences.getInstance();
    if (prefs.getString(_sessionIdKey) == sessionId) {
      await prefs.setString(_lastEventIdKey, lastEventId);
    }
  }

  ResourcePreview _resourcePreviewFromJson(
      Map<String, Object?> json, String uri) {
    return ResourcePreview(
      uri: json['uri'] as String? ?? uri,
      kind: json['kind'] as String? ?? '',
      mime: json['mime'] as String? ?? '',
      title: json['title'] as String?,
      sizeBytes: json['size_bytes'] as int? ?? json['sizeBytes'] as int? ?? 0,
      preview: json['preview'] as String? ?? '',
      content: json['content'] as String? ?? '',
      hasMore: json['has_more'] as bool? ?? json['hasMore'] as bool? ?? false,
    );
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

  SessionSummary _summaryFromJson(Map<String, Object?> json) {
    return SessionSummary(
      id: json['id'] as String? ?? '',
      title: json['title'] as String? ?? 'New session',
      preview: json['preview'] as String? ?? '',
      mode: json['mode'] as String? ?? '',
      label: json['label'] as String? ?? '',
      updatedAt: (json['updatedAt'] as String?) ??
          (json['updated_at'] as String?) ??
          '',
      status: json['status'] as String? ?? '',
      messageCount: (json['messageCount'] as int?) ??
          (json['message_count'] as int?) ??
          0,
      lastEventId: _nullableString(
        json['lastEventId'] ?? json['last_event_id'],
      ),
    );
  }

  SessionMessage _messageFromJson(Map<String, Object?> json) {
    return SessionMessage(
      seq: (json['seq'] as num?)?.toInt() ?? 0,
      role: json['role'] as String? ?? '',
      content: json['content'] as String? ?? '',
      createdAt: (json['createdAt'] as String?) ??
          (json['created_at'] as String?) ??
          '',
    );
  }

  MikuEvent _eventFromJson(Map<String, Object?> json) {
    final data = (json['data'] as Map?)?.cast<String, Object?>() ??
        const <String, Object?>{};
    return MikuEvent(
      type: json['type'] as String? ?? '',
      id: _nullableString(json['id']),
      data: data,
    );
  }

  String _normalizeServerBaseUrl(String value) {
    var text = value.trim();
    if (text.isEmpty) {
      throw const FormatException('server target is empty');
    }
    if (!text.contains('://')) {
      text = 'http://$text';
    }
    final uri = Uri.parse(text);
    if (!uri.hasScheme || uri.host.isEmpty) {
      throw const FormatException('server target must include a host');
    }
    final normalized = uri.replace(query: '', fragment: '').toString();
    return normalized.endsWith('/')
        ? normalized.substring(0, normalized.length - 1)
        : normalized;
  }

  String? _nullableString(Object? value) {
    final text = value?.toString() ?? '';
    return text.isEmpty ? null : text;
  }
}
