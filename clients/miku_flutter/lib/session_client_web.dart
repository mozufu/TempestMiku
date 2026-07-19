// ignore_for_file: avoid_web_libraries_in_flutter, deprecated_member_use

import 'dart:html';
import 'dart:async';
import 'dart:convert';
import 'dart:js_util' as js_util;
import 'dart:typed_data';

import 'session_models.dart';
import 'session_sse.dart';

MikuSessionClient createClient() => WebMikuSessionClient();

class _AmbiguousWebTransportFailure implements Exception {
  const _AmbiguousWebTransportFailure();
}

class WebMikuSessionClient implements MikuSessionClient {
  static const _sessionIdKey = 'tempestmiku.sessionId';
  static const _lastEventIdKey = 'tempestmiku.lastEventId';

  @override
  Future<VoiceAsrEngineCatalog> voiceAsrEngines() async {
    final json = await _request('GET', '/voice/asr/engines');
    return VoiceAsrEngineCatalog.fromJson(json);
  }

  @override
  Future<VoiceAsrTranscript> transcribeVoicePcm16({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) async {
    validateVoiceAsrPcm16Request(
      engineId: engineId,
      captureId: captureId,
      sampleRate: sampleRate,
      pcm16: pcm16,
    );
    late final HttpRequest response;
    try {
      response = await HttpRequest.request(
        '/voice/asr/transcriptions',
        method: 'POST',
        requestHeaders: {
          'content-type': 'application/octet-stream',
          'accept': 'application/json',
          'x-tm-asr-engine-id': engineId,
          'x-tm-capture-id': captureId,
          'x-tm-sample-rate': '$sampleRate',
          'x-tm-channels': '$voiceAsrChannels',
        },
        sendData: pcm16,
      );
    } catch (_) {
      throw const _AmbiguousWebTransportFailure();
    }
    final status = response.status ?? 0;
    if (status == 0) throw const _AmbiguousWebTransportFailure();
    if (status < 200 || status >= 300) {
      throw StateError('request failed: $status ${response.responseText}');
    }
    final text = response.responseText;
    if (text == null || text.isEmpty) {
      throw const FormatException('voice ASR returned an empty response');
    }
    return VoiceAsrTranscript.fromJson(
      (jsonDecode(text) as Map).cast<String, Object?>(),
    );
  }

  @override
  Future<void> cancelVoiceAsrTranscription() async {
    // Browser voice capture is currently unsupported. Keep the shared client
    // contract explicit without inventing a second web cancellation path.
  }

  @override
  Future<ModeCatalog> modeCatalog() async {
    final json = await _request('GET', '/modes');
    return ModeCatalog.fromJson(json);
  }

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
    _rememberSession(session);
    final messages =
        ((json['messages'] as List?) ?? const [])
            .whereType<Map>()
            .map((item) => _messageFromJson(item.cast<String, Object?>()))
            .toList();
    final pendingEvents =
        ((json['pendingEvents'] as List?) ??
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
    var closed = false;
    Object? activeAbortController;
    controller.onCancel = () {
      closed = true;
      final abortController = activeAbortController;
      if (abortController != null) {
        js_util.callMethod<void>(abortController, 'abort', const []);
      }
    };
    unawaited(
      _pumpEvents(
        controller,
        () => closed || controller.isClosed,
        (value) => activeAbortController = value,
        sessionId,
        lastEventId,
      ),
    );
    return controller.stream;
  }

  Future<void> _pumpEvents(
    StreamController<MikuEvent> controller,
    bool Function() isClosed,
    void Function(Object? controller) setAbortController,
    String sessionId,
    String? initialLastEventId,
  ) async {
    var resumeId = initialLastEventId ?? window.localStorage[_lastEventIdKey];
    if (numericEventId(resumeId) == null) resumeId = null;
    final lifecycle = SessionEventLifecycle(resumeId);

    while (!isClosed() && lifecycle.shouldReconnect) {
      try {
        final abortController = _newAbortController();
        setAbortController(abortController);
        final decoder = SessionEventSseDecoder();
        eventStream:
        await for (final chunk in _fetchEventChunks(
          sessionId,
          resumeId,
          abortController,
          () {
            if (!isClosed()) {
              controller.add(
                const MikuEvent(
                  type: 'connection',
                  data: {'status': 'connected'},
                ),
              );
            }
          },
        )) {
          for (final event in decoder.add(chunk)) {
            if (isClosed()) break;
            if (!lifecycle.accept(event)) continue;
            final eventId = event.id!;
            if (shouldRememberEventId(event.type, event.data)) {
              resumeId = eventId;
              rememberLastEventId(sessionId, eventId);
            }
            controller.add(event);
            if (lifecycle.isTerminal) break eventStream;
          }
          if (isClosed()) break;
        }
        if (!isClosed() && !lifecycle.isTerminal) {
          for (final event in decoder.close()) {
            if (!lifecycle.accept(event)) continue;
            final eventId = event.id!;
            if (shouldRememberEventId(event.type, event.data)) {
              resumeId = eventId;
              rememberLastEventId(sessionId, eventId);
            }
            controller.add(event);
            if (lifecycle.isTerminal) break;
          }
        }
      } catch (_) {
        if (!isClosed() && lifecycle.shouldReconnect) {
          controller.add(
            const MikuEvent(
              type: 'connection',
              data: {'status': 'reconnecting'},
            ),
          );
          await Future<void>.delayed(const Duration(seconds: 2));
        }
      } finally {
        setAbortController(null);
      }
    }
    if (lifecycle.isTerminal && !controller.isClosed) {
      await controller.close();
    }
  }

  Object _newAbortController() {
    final constructor = js_util.getProperty<Object>(window, 'AbortController');
    return js_util.callConstructor<Object>(constructor, const []);
  }

  Stream<String> _fetchEventChunks(
    String sessionId,
    String? lastEventId,
    Object abortController,
    void Function() onOpen,
  ) async* {
    final suffix =
        lastEventId == null
            ? ''
            : '?lastEventId=${Uri.encodeQueryComponent(lastEventId)}';
    final signal = js_util.getProperty<Object>(abortController, 'signal');
    final init = js_util.jsify({
      'method': 'GET',
      'credentials': 'same-origin',
      'cache': 'no-store',
      'headers': {
        'accept': 'text/event-stream',
        'cache-control': 'no-cache',
        if (lastEventId != null) 'Last-Event-ID': lastEventId,
      },
      'signal': signal,
    });
    final promise = js_util.callMethod<Object>(window, 'fetch', [
      '/sessions/$sessionId/events$suffix',
      init,
    ]);
    final response = await js_util.promiseToFuture<Object>(promise);
    final status = js_util.getProperty<num>(response, 'status').toInt();
    if (status < 200 || status >= 300) {
      throw StateError('event stream failed: $status');
    }
    final body = js_util.getProperty<Object?>(response, 'body');
    if (body == null) throw StateError('streaming fetch body is unavailable');
    onOpen();
    final reader = js_util.callMethod<Object>(body, 'getReader', const []);
    final textDecoderConstructor = js_util.getProperty<Object>(
      window,
      'TextDecoder',
    );
    final textDecoder = js_util.callConstructor<Object>(
      textDecoderConstructor,
      ['utf-8'],
    );
    while (true) {
      final readPromise = js_util.callMethod<Object>(reader, 'read', const []);
      final result = await js_util.promiseToFuture<Object>(readPromise);
      if (js_util.getProperty<bool>(result, 'done')) break;
      final value = js_util.getProperty<Object>(result, 'value');
      final chunk = js_util.callMethod<String>(textDecoder, 'decode', [
        value,
        js_util.jsify({'stream': true}),
      ]);
      if (chunk.isNotEmpty) yield chunk;
    }
    final tail = js_util.callMethod<String>(textDecoder, 'decode', const []);
    if (tail.isNotEmpty) yield tail;
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    if (window.localStorage[_sessionIdKey] == sessionId) {
      window.localStorage[_lastEventIdKey] = lastEventId;
    }
  }

  @override
  Future<void> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) async {
    await sendIdempotentMessageWithRetry(
      clientMessageId: clientMessageId,
      isAmbiguousFailure: (error) => error is _AmbiguousWebTransportFailure,
      send: (stableClientMessageId) async {
        await _request(
          'POST',
          '/sessions/$sessionId/messages',
          body: {'clientMessageId': stableClientMessageId, 'content': content},
        );
      },
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
      body: {'decision': decision, if (optionId != null) 'optionId': optionId},
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
      nextActions:
          ((json['nextActions'] as List?) ?? const [])
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
    final query =
        Uri(
          queryParameters: {
            'limit': '$limit',
            if (trimmedProject != null && trimmedProject.isNotEmpty)
              'project': trimmedProject,
          },
        ).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/drive/feed?$query',
    );
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

  ResourcePreview _resourcePreviewFromJson(
    Map<String, Object?> json,
    String uri,
  ) {
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
    late final HttpRequest response;
    try {
      response = await HttpRequest.request(
        path,
        method: method,
        requestHeaders: {if (body != null) 'content-type': 'application/json'},
        sendData: body == null ? null : jsonEncode(body),
      );
    } catch (_) {
      throw const _AmbiguousWebTransportFailure();
    }
    final status = response.status ?? 0;
    if (status == 0) throw const _AmbiguousWebTransportFailure();
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
    final modeState =
        (json['mode_state'] as Map?)?.cast<String, Object?>() ??
        (json['modeState'] as Map?)?.cast<String, Object?>() ??
        const <String, Object?>{};
    return MikuSession(
      id: json['id'] as String,
      status: json['status'] as String? ?? 'active',
      mode: (json['mode'] as String?) ?? (modeState['mode'] as String?) ?? '',
      label: json['label'] as String? ?? '',
      voiceCap:
          (json['voice_cap'] as String?) ?? (json['voiceCap'] as String?) ?? '',
      defaultScope:
          (json['default_scope'] as String?) ??
          (json['defaultScope'] as String?) ??
          'global',
      activeSkills:
          ((json['activeSkills'] as List?) ??
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
      updatedAt:
          (json['updatedAt'] as String?) ??
          (json['updated_at'] as String?) ??
          '',
      status: json['status'] as String? ?? '',
      messageCount:
          (json['messageCount'] as int?) ??
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
      createdAt:
          (json['createdAt'] as String?) ??
          (json['created_at'] as String?) ??
          '',
    );
  }

  MikuEvent _eventFromJson(Map<String, Object?> json) {
    final data =
        (json['data'] as Map?)?.cast<String, Object?>() ??
        const <String, Object?>{};
    return MikuEvent(
      type: json['type'] as String? ?? '',
      id: _nullableString(json['id']),
      data: data,
    );
  }

  String? _nullableString(Object? value) {
    final text = value?.toString() ?? '';
    return text.isEmpty ? null : text;
  }

  void _rememberSession(MikuSession session) {
    window.localStorage[_sessionIdKey] = session.id;
    if (session.lastEventId != null && session.lastEventId!.isNotEmpty) {
      window.localStorage[_lastEventIdKey] = session.lastEventId!;
    } else {
      window.localStorage.remove(_lastEventIdKey);
    }
  }
}
