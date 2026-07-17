import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'session_models.dart';
import 'session_sse.dart';

part 'session_client_io/auth.dart';
part 'session_client_io/credentials.dart';
part 'session_client_io/drive.dart';
part 'session_client_io/events.dart';
part 'session_client_io/sessions.dart';
part 'session_client_io/transport.dart';

MikuSessionClient createClient() => NativeMikuSessionClient();

class NativeMikuSessionClient
    implements
        MikuSessionClient,
        ServerTargetClient,
        PushRegistrationClient,
        NotificationReplyAuthorityClient {
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
  String pairingDeviceName() => _pairingDeviceNameImpl();

  @override
  Future<String> serverBaseUrl() => _serverBaseUrlImpl();

  @override
  Future<void> setServerBaseUrl(String baseUrl) =>
      _setServerBaseUrlImpl(baseUrl);

  @override
  Future<void> pairWithCode(MikuPairingTarget target) =>
      _pairWithCodeImpl(target);

  @override
  Future<void> logout() => _logoutImpl();

  @override
  Future<bool> hasDeviceCredential() => _hasDeviceCredentialImpl();

  @override
  Future<NotificationReplyAuthority?> notificationReplyAuthority() =>
      _notificationReplyAuthorityImpl();

  @override
  Future<void> registerPush({
    required String endpoint,
    required String p256dh,
    required String auth,
  }) => _registerPushImpl(endpoint: endpoint, p256dh: p256dh, auth: auth);

  @override
  Future<void> unregisterPush() => _unregisterPushImpl();

  @override
  Future<ModeCatalog> modeCatalog() => _modeCatalogImpl();

  @override
  Future<MikuSession> createOrReuseSession() => _createOrReuseSessionImpl();

  @override
  Future<MikuSession> createSession() => _createSessionImpl();

  @override
  Future<List<SessionSummary>> listSessions({int limit = 30}) =>
      _listSessionsImpl(limit: limit);

  @override
  Future<LoadedSession> loadSession(String sessionId) =>
      _loadSessionImpl(sessionId);

  @override
  Stream<MikuEvent> events(String sessionId, {String? lastEventId}) =>
      _eventsImpl(sessionId, lastEventId: lastEventId);

  @override
  void rememberLastEventId(String sessionId, String lastEventId) =>
      _rememberLastEventIdImpl(sessionId, lastEventId);

  @override
  Future<void> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) => _sendMessageImpl(sessionId, content, clientMessageId: clientMessageId);

  @override
  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  }) =>
      _resolveApprovalImpl(sessionId, approvalId, decision, optionId: optionId);

  @override
  Future<void> lockMode(String sessionId, String mode) =>
      _lockModeImpl(sessionId, mode);

  @override
  Future<void> unlockMode(String sessionId) => _unlockModeImpl(sessionId);

  @override
  Future<void> overrideMode(String sessionId, String mode) =>
      _overrideModeImpl(sessionId, mode);

  @override
  Future<ProjectOverview> projectOverview(String sessionId) =>
      _projectOverviewImpl(sessionId);

  @override
  Future<DriveFeed> driveFeed(
    String sessionId, {
    int limit = 20,
    String? project,
  }) => _driveFeedImpl(sessionId, limit: limit, project: project);

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) =>
      _previewResourceImpl(sessionId, uri);

  @override
  Future<ResourcePreview> resolveResource(String sessionId, String uri) =>
      _resolveResourceImpl(sessionId, uri);

  @override
  Future<ProjectPromotion> promoteSession(
    String sessionId, {
    String? summary,
    List<String> openLoops = const [],
    List<String> decisions = const [],
    List<String> resources = const [],
  }) => _promoteSessionImpl(
    sessionId,
    summary: summary,
    openLoops: openLoops,
    decisions: decisions,
    resources: resources,
  );

  @visibleForTesting
  Future<String?> deviceTokenForTesting() => _deviceToken();
}
