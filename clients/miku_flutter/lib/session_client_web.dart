import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

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
    late final web.Response response;
    try {
      response = await _fetch(
        'POST',
        '/voice/asr/transcriptions',
        headers: {
          'content-type': 'application/octet-stream',
          'accept': 'application/json',
          'x-tm-asr-engine-id': engineId,
          'x-tm-capture-id': captureId,
          'x-tm-sample-rate': '$sampleRate',
          'x-tm-channels': '$voiceAsrChannels',
        },
        body: pcm16.toJS,
      );
    } catch (_) {
      throw const _AmbiguousWebTransportFailure();
    }
    final status = response.status;
    if (status == 0) throw const _AmbiguousWebTransportFailure();
    if (status < 200 || status >= 300) {
      throw StateError(
        'request failed: $status ${await _responseText(response)}',
      );
    }
    final text = await _responseText(response);
    if (text.isEmpty) {
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
    final storedId = web.window.localStorage.getItem(_sessionIdKey);
    if (storedId != null && storedId.isNotEmpty) {
      try {
        final json = await _request('GET', '/sessions/$storedId');
        final session = _sessionFromJson(
          json,
          lastEventId: web.window.localStorage.getItem(_lastEventIdKey),
        );
        _rememberSession(session);
        return session;
      } catch (_) {
        web.window.localStorage.removeItem(_sessionIdKey);
        web.window.localStorage.removeItem(_lastEventIdKey);
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
    web.AbortController? activeAbortController;
    controller.onCancel = () {
      closed = true;
      final abortController = activeAbortController;
      abortController?.abort();
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
    void Function(web.AbortController? controller) setAbortController,
    String sessionId,
    String? initialLastEventId,
  ) async {
    var resumeId =
        initialLastEventId ?? web.window.localStorage.getItem(_lastEventIdKey);
    if (numericEventId(resumeId) == null) resumeId = null;
    final lifecycle = SessionEventLifecycle(resumeId);

    while (!isClosed() && lifecycle.shouldReconnect) {
      try {
        final abortController = web.AbortController();
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

  Stream<String> _fetchEventChunks(
    String sessionId,
    String? lastEventId,
    web.AbortController abortController,
    void Function() onOpen,
  ) async* {
    final suffix =
        lastEventId == null
            ? ''
            : '?lastEventId=${Uri.encodeQueryComponent(lastEventId)}';
    final headers =
        web.Headers()
          ..set('accept', 'text/event-stream')
          ..set('cache-control', 'no-cache');
    if (lastEventId != null) headers.set('Last-Event-ID', lastEventId);
    final response =
        await web.window
            .fetch(
              '/sessions/$sessionId/events$suffix'.toJS,
              web.RequestInit(
                method: 'GET',
                credentials: 'same-origin',
                cache: 'no-store',
                headers: headers,
                signal: abortController.signal,
              ),
            )
            .toDart;
    final status = response.status;
    if (status < 200 || status >= 300) {
      throw StateError('event stream failed: $status');
    }
    final body = response.body;
    if (body == null) throw StateError('streaming fetch body is unavailable');
    onOpen();
    final reader = body.getReader() as web.ReadableStreamDefaultReader;
    final textDecoder = web.TextDecoder('utf-8');
    while (true) {
      final result = await reader.read().toDart;
      if (result.done) break;
      final value = result.value;
      if (value == null) continue;
      final chunk = textDecoder.decode(
        value as JSObject,
        web.TextDecodeOptions(stream: true),
      );
      if (chunk.isNotEmpty) yield chunk;
    }
    final tail = textDecoder.decode();
    if (tail.isNotEmpty) yield tail;
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    if (web.window.localStorage.getItem(_sessionIdKey) == sessionId) {
      web.window.localStorage.setItem(_lastEventIdKey, lastEventId);
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
    late final web.Response response;
    try {
      response = await _fetch(
        method,
        path,
        headers: {if (body != null) 'content-type': 'application/json'},
        body: body == null ? null : jsonEncode(body).toJS,
      );
    } catch (_) {
      throw const _AmbiguousWebTransportFailure();
    }
    final status = response.status;
    if (status == 0) throw const _AmbiguousWebTransportFailure();
    if (status < 200 || status >= 300) {
      throw StateError(
        'request failed: $status ${await _responseText(response)}',
      );
    }
    final text = await _responseText(response);
    if (text.isEmpty) return <String, Object?>{};
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
    web.window.localStorage.setItem(_sessionIdKey, session.id);
    if (session.lastEventId != null && session.lastEventId!.isNotEmpty) {
      web.window.localStorage.setItem(_lastEventIdKey, session.lastEventId!);
    } else {
      web.window.localStorage.removeItem(_lastEventIdKey);
    }
  }

  Future<web.Response> _fetch(
    String method,
    String path, {
    Map<String, String> headers = const {},
    web.BodyInit? body,
  }) {
    final requestHeaders = web.Headers();
    for (final MapEntry(:key, :value) in headers.entries) {
      requestHeaders.set(key, value);
    }
    return web.window
        .fetch(
          path.toJS,
          web.RequestInit(
            method: method,
            credentials: 'same-origin',
            headers: requestHeaders,
            body: body,
          ),
        )
        .toDart;
  }

  Future<String> _responseText(web.Response response) async =>
      (await response.text().toDart).toDart;
}
