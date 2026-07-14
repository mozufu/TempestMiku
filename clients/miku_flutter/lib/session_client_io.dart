import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'session_models.dart';
import 'session_sse.dart';

MikuSessionClient createClient() => NativeMikuSessionClient();

class DeviceCredential {
  const DeviceCredential({required this.serverBaseUrl, required this.token});

  final String serverBaseUrl;
  final String token;

  String encode() => jsonEncode({
    'version': 1,
    'serverBaseUrl': serverBaseUrl,
    'token': token,
  });

  static DeviceCredential? decode(String? value) {
    if (value == null || value.isEmpty) return null;
    try {
      final json = jsonDecode(value);
      if (json is! Map || json['version'] != 1) return null;
      final serverBaseUrl = json['serverBaseUrl'];
      final token = json['token'];
      if (serverBaseUrl is! String ||
          serverBaseUrl.isEmpty ||
          token is! String ||
          !token.startsWith('tmk_dev_')) {
        return null;
      }
      return DeviceCredential(serverBaseUrl: serverBaseUrl, token: token);
    } catch (_) {
      return null;
    }
  }
}

abstract class DeviceTokenStore {
  Future<DeviceCredential?> read();

  Future<void> write(DeviceCredential credential);

  Future<void> delete();
}

class SecureDeviceTokenStore implements DeviceTokenStore {
  SecureDeviceTokenStore({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage();

  static const _key = 'tempestmiku.deviceCredential.v1';
  static const _legacyUnboundKey = 'tempestmiku.deviceToken';
  final FlutterSecureStorage _storage;

  @override
  Future<DeviceCredential?> read() async =>
      DeviceCredential.decode(await _storage.read(key: _key));

  @override
  Future<void> write(DeviceCredential credential) async {
    await _storage.delete(key: _legacyUnboundKey);
    await _storage.write(key: _key, value: credential.encode());
  }

  @override
  Future<void> delete() async {
    await _storage.delete(key: _key);
    await _storage.delete(key: _legacyUnboundKey);
  }
}

class MemoryDeviceTokenStore implements DeviceTokenStore {
  DeviceCredential? credential;

  @override
  Future<void> delete() async => credential = null;

  @override
  Future<DeviceCredential?> read() async => credential;

  @override
  Future<void> write(DeviceCredential credential) async =>
      this.credential = credential;
}

class NativeMikuSessionClient
    implements MikuSessionClient, ServerTargetClient, PushRegistrationClient {
  NativeMikuSessionClient({DeviceTokenStore? tokenStore})
    : _tokenStore = tokenStore ?? SecureDeviceTokenStore();

  static const _serverBaseUrlKey = 'tempestmiku.serverBaseUrl';
  static const _sessionIdKey = 'tempestmiku.sessionId';
  static const _lastEventIdKey = 'tempestmiku.lastEventId';
  static const _configuredServerBaseUrl = String.fromEnvironment(
    'MIKU_SERVER_URL',
  );
  static const _defaultServerBaseUrl = 'http://10.0.2.2:8787';

  final HttpClient _http = HttpClient();
  final DeviceTokenStore _tokenStore;
  DeviceCredential? _cachedCredential;
  bool _tokenLoaded = false;

  @override
  String pairingDeviceName() => 'TempestMiku ${Platform.operatingSystem}';

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
    if (kReleaseMode) {
      throw StateError('this device is not securely paired');
    }
    return _defaultServerBaseUrl;
  }

  @override
  Future<void> setServerBaseUrl(String baseUrl) async {
    final normalized = _normalizeServerBaseUrl(baseUrl);
    final prefs = await SharedPreferences.getInstance();
    final previous = prefs.getString(_serverBaseUrlKey);
    final previousNormalized =
        previous == null ? null : _tryNormalizeServerBaseUrl(previous);
    if (previousNormalized != normalized) {
      // Publish the new target only after all authority and state for the previous origin is gone.
      // A crash at any await before setString therefore leaves the old target unauthenticated.
      await _clearDeviceToken();
      await prefs.remove(_sessionIdKey);
      await prefs.remove(_lastEventIdKey);
      await prefs.setString(_serverBaseUrlKey, normalized);
    }
  }

  @override
  Future<void> pairWithCode(MikuPairingTarget target) async {
    final normalized = _normalizeServerBaseUrl(target.serverBaseUrl);
    final json = await _pairRequest(normalized, <String, Object?>{
      'code': target.code,
      'deviceName': pairingDeviceName(),
      'platform': Platform.operatingSystem,
    });
    final token = json['token']?.toString().trim();
    if (token == null || !token.startsWith('tmk_dev_')) {
      throw const FormatException(
        'pairing response did not include a device token',
      );
    }
    final prefs = await SharedPreferences.getInstance();
    final previous = prefs.getString(_serverBaseUrlKey);
    final previousNormalized =
        previous == null ? null : _tryNormalizeServerBaseUrl(previous);
    if (previousNormalized != null) {
      // The connector endpoint is stable across app registrations. Retire the old device's
      // server-side route before publishing the replacement credential so an old pairing cannot
      // continue targeting this installation.
      await _unregisterPushBestEffort();
    }
    if (previousNormalized != normalized) {
      await _clearDeviceToken();
    }
    await prefs.remove(_sessionIdKey);
    await prefs.remove(_lastEventIdKey);
    final credential = DeviceCredential(
      serverBaseUrl: normalized,
      token: token,
    );
    await _tokenStore.write(credential);
    _cachedCredential = credential;
    _tokenLoaded = true;
    // The origin-bound credential is safe if a crash occurs before this final publication: it
    // cannot authenticate requests to the still-selected old origin.
    await prefs.setString(_serverBaseUrlKey, normalized);
  }

  @override
  Future<void> logout() async {
    try {
      await _request('POST', '/auth/logout');
    } finally {
      await _clearDeviceToken();
      final prefs = await SharedPreferences.getInstance();
      await prefs.remove(_sessionIdKey);
      await prefs.remove(_lastEventIdKey);
    }
  }

  @override
  Future<bool> hasDeviceCredential() async {
    try {
      return (await _deviceToken()) != null;
    } catch (_) {
      return false;
    }
  }

  @override
  Future<void> registerPush({
    required String endpoint,
    required String p256dh,
    required String auth,
  }) async {
    await _request(
      'PUT',
      '/auth/push-registration',
      body: {
        'provider': 'unifiedpush',
        'registration': jsonEncode({
          'endpoint': endpoint,
          'p256dh': p256dh,
          'auth': auth,
        }),
      },
    );
  }

  @override
  Future<void> unregisterPush() async {
    await _request('DELETE', '/auth/push-registration');
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
    final eventClient = HttpClient();
    var closed = false;
    controller.onCancel = () {
      closed = true;
      eventClient.close(force: true);
    };
    unawaited(
      _pumpEvents(
        controller,
        eventClient,
        () => closed || controller.isClosed,
        sessionId,
        lastEventId,
      ),
    );
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
    if (numericEventId(resumeId) == null) resumeId = null;
    final lifecycle = SessionEventLifecycle(resumeId);
    while (!isClosed() && lifecycle.shouldReconnect) {
      try {
        final baseUrl = await serverBaseUrl();
        final request = await eventClient.getUrl(
          _resolveAgainst(baseUrl, _eventsPath(sessionId, resumeId)),
        );
        request.headers
          ..set(HttpHeaders.acceptHeader, 'text/event-stream')
          ..set(HttpHeaders.cacheControlHeader, 'no-cache');
        final token = await _deviceToken(requestBaseUrl: baseUrl);
        if (token != null) {
          request.headers.set(HttpHeaders.authorizationHeader, 'Bearer $token');
        }
        if (resumeId != null && resumeId.isNotEmpty) {
          request.headers.set('Last-Event-ID', resumeId);
        }
        final response = await request.close();
        if (response.statusCode < 200 || response.statusCode >= 300) {
          throw StateError('event stream failed: ${response.statusCode}');
        }
        if (!isClosed()) {
          controller.add(
            const MikuEvent(type: 'connection', data: {'status': 'connected'}),
          );
        }
        final decoder = SessionEventSseDecoder();
        eventStream:
        await for (final chunk in response.transform(utf8.decoder)) {
          for (final event in decoder.add(chunk)) {
            if (isClosed()) break;
            if (!lifecycle.accept(event)) continue;
            final eventId = event.id!;
            if (shouldRememberEventId(event.type, event.data)) {
              resumeId = eventId;
              await _rememberLastEventId(sessionId, eventId);
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
              await _rememberLastEventId(sessionId, eventId);
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
      }
    }
    eventClient.close(force: true);
    if (lifecycle.isTerminal && !controller.isClosed) {
      await controller.close();
    }
  }

  @override
  void rememberLastEventId(String sessionId, String lastEventId) {
    unawaited(_rememberLastEventId(sessionId, lastEventId));
  }

  @override
  Future<void> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) async {
    await sendIdempotentMessageWithRetry(
      clientMessageId: clientMessageId,
      isAmbiguousFailure:
          (error) => error is IOException || error is TimeoutException,
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
    final baseUrl = await serverBaseUrl();
    final request = await _http.openUrl(method, _resolveAgainst(baseUrl, path));
    request.headers.set(HttpHeaders.acceptHeader, 'application/json');
    final token = await _deviceToken(requestBaseUrl: baseUrl);
    if (token != null) {
      request.headers.set(HttpHeaders.authorizationHeader, 'Bearer $token');
    }
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

  Future<Map<String, Object?>> _pairRequest(
    String baseUrl,
    Map<String, Object?> body,
  ) async {
    final baseUri = Uri.parse(baseUrl.endsWith('/') ? baseUrl : '$baseUrl/');
    final request = await _http.postUrl(baseUri.resolve('auth/pair'));
    request.headers
      ..set(HttpHeaders.acceptHeader, 'application/json')
      ..contentType = ContentType.json;
    request.write(jsonEncode(body));
    final response = await request.close();
    final text = await response.transform(utf8.decoder).join();
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw StateError('pairing failed: ${response.statusCode} $text');
    }
    return (jsonDecode(text) as Map).cast<String, Object?>();
  }

  Future<String?> _deviceToken({String? requestBaseUrl}) async {
    if (!_tokenLoaded) {
      _cachedCredential = await _tokenStore.read();
      _tokenLoaded = true;
    }
    final selectedServer = _normalizeServerBaseUrl(await serverBaseUrl());
    if (requestBaseUrl != null &&
        _normalizeServerBaseUrl(requestBaseUrl) != selectedServer) {
      return null;
    }
    final credential = _cachedCredential;
    if (credential == null || credential.serverBaseUrl != selectedServer) {
      return null;
    }
    final token = credential.token.trim();
    return token.isEmpty ? null : token;
  }

  @visibleForTesting
  Future<String?> deviceTokenForTesting() => _deviceToken();

  Future<void> _clearDeviceToken() async {
    await _tokenStore.delete();
    _cachedCredential = null;
    _tokenLoaded = true;
  }

  Future<void> _unregisterPushBestEffort() async {
    try {
      await unregisterPush();
    } catch (_) {
      // Re-pairing must still work when the previous server is offline or already revoked.
    }
  }

  Uri _resolveAgainst(String base, String path) {
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

  String _normalizeServerBaseUrl(String value) {
    return normalizeMikuServerBaseUrl(value);
  }

  String? _tryNormalizeServerBaseUrl(String value) {
    try {
      return _normalizeServerBaseUrl(value);
    } catch (_) {
      return null;
    }
  }

  String? _nullableString(Object? value) {
    final text = value?.toString() ?? '';
    return text.isEmpty ? null : text;
  }
}
