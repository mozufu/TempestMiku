import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'session_models.dart';
import 'session_sse.dart';

part 'session_client_io/auth.dart';
part 'session_client_io/credentials.dart';
part 'session_client_io/drive.dart';
part 'session_client_io/events.dart';
part 'session_client_io/proposals.dart';
part 'session_client_io/sessions.dart';
part 'session_client_io/settings.dart';
part 'session_client_io/transport.dart';
part 'session_client_io/voice_asr.dart';

MikuSessionClient createClient() => NativeMikuSessionClient();

class NativeMikuSessionClient
    implements
        MikuSessionClient,
        ServerTargetClient,
        CurrentAuthDeviceClient,
        PushRegistrationClient,
        NotificationReplyAuthorityClient {
  NativeMikuSessionClient({
    DeviceTokenStore? tokenStore,
    this.voiceAsrRequestTimeout = const Duration(seconds: 50),
    this.openVoiceAsrRequestForTesting,
  }) : _tokenStore = tokenStore ?? SecureDeviceTokenStore();

  static const _serverBaseUrlKey = 'tempestmiku.serverBaseUrl';
  static const _sessionIdKey = 'tempestmiku.sessionId';
  static const _lastEventIdKey = 'tempestmiku.lastEventId';
  static const _configuredServerBaseUrl = String.fromEnvironment(
    'MIKU_SERVER_URL',
  );
  static const _defaultServerBaseUrl = 'http://10.0.2.2:8787';

  final HttpClient _http = HttpClient();
  final DeviceTokenStore _tokenStore;
  final Duration voiceAsrRequestTimeout;
  @visibleForTesting
  final Future<HttpClientRequest> Function(String method, Uri uri)?
  openVoiceAsrRequestForTesting;
  DeviceCredential? _cachedCredential;
  bool _tokenLoaded = false;
  bool _voiceAsrRequestActive = false;
  int _voiceAsrRequestEpoch = 0;
  HttpClientRequest? _activeVoiceAsrRequest;
  Completer<void>? _activeVoiceAsrDone;
  Completer<Map<String, Object?>>? _activeVoiceAsrCancellation;

  @override
  String pairingDeviceName() => _pairingDeviceNameImpl();

  @override
  Future<String> serverBaseUrl() => _serverBaseUrlImpl();

  @override
  Future<ServerReadiness> serverReadiness() => _serverReadinessImpl();

  @override
  Future<void> setServerBaseUrl(String baseUrl) =>
      _setServerBaseUrlImpl(baseUrl);

  @override
  Future<void> pairWithCode(MikuPairingTarget target) =>
      _pairWithCodeImpl(target);

  @override
  Future<void> logout() => _logoutImpl();

  @override
  Future<String?> currentAuthDeviceId() => _currentAuthDeviceIdImpl();

  @override
  Future<ServerDiagnostics> serverDiagnostics() => _serverDiagnosticsImpl();

  @override
  Future<List<AuthDevice>> authDevices() => _authDevicesImpl();

  @override
  Future<PairingCode> createPairingCode() => _createPairingCodeImpl();

  @override
  Future<void> revokeAuthDevice(String deviceId) =>
      _revokeAuthDeviceImpl(deviceId);

  @override
  Future<bool> hasDeviceCredential() => _hasDeviceCredentialImpl();

  @override
  Future<NotificationReplyAuthority?> notificationReplyAuthority() =>
      _notificationReplyAuthorityImpl();

  @override
  Future<PushRegistrationMetadata> registerPush({
    required String endpoint,
    required String p256dh,
    required String auth,
  }) => _registerPushImpl(endpoint: endpoint, p256dh: p256dh, auth: auth);

  @override
  Future<void> unregisterPush() => _unregisterPushImpl();

  @override
  Future<VoiceAsrEngineCatalog> voiceAsrEngines() => _voiceAsrEnginesImpl();

  @override
  Future<VoiceAsrTranscript> transcribeVoicePcm16({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  }) => _transcribeVoicePcm16Impl(
    engineId: engineId,
    captureId: captureId,
    sampleRate: sampleRate,
    pcm16: pcm16,
  );

  @override
  Future<void> cancelVoiceAsrTranscription() =>
      _cancelVoiceAsrTranscriptionImpl();

  @override
  Future<ModeCatalog> modeCatalog() => _modeCatalogImpl();

  @override
  Future<MikuSession> createOrReuseSession() => _createOrReuseSessionImpl();

  @override
  Future<MikuSession> createSession() => _createSessionImpl();

  @override
  Future<void> endSession(String sessionId) => _endSessionImpl(sessionId);

  @override
  Future<List<SessionSummary>> listSessions({int limit = 30}) =>
      _listSessionsImpl(limit: limit);

  @override
  Future<List<ProjectCatalogEntry>> listProjects() => _listProjectsImpl();

  @override
  Future<String> setSessionScope(String sessionId, String scope) =>
      _setSessionScopeImpl(sessionId, scope);

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
  Future<TurnReceipt> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  }) => _sendMessageImpl(sessionId, content, clientMessageId: clientMessageId);

  @override
  Future<SessionTurn> getTurn(String sessionId, String turnId) =>
      _getTurnImpl(sessionId, turnId);

  @override
  Future<MemoryWriteProposalResult> proposeMemoryWrite(
    String sessionId,
    MemoryWriteProposalRequest request,
  ) => _proposeMemoryWriteImpl(sessionId, request);

  @override
  Future<EvolutionReviewProposalResult> proposeEvolutionReview(
    String sessionId,
    EvolutionReviewProposalRequest request,
  ) => _proposeEvolutionReviewImpl(sessionId, request);

  @override
  Future<ModeAddendumRollbackResult> proposeModeAddendumRollback(
    String sessionId,
    String modeId,
    AddendumRollbackRequest request,
  ) => _proposeModeAddendumRollbackImpl(sessionId, modeId, request);

  @override
  Future<PersonaAddendumRollbackResult> proposePersonaAddendumRollback(
    String sessionId,
    String personaId,
    AddendumRollbackRequest request,
  ) => _proposePersonaAddendumRollbackImpl(sessionId, personaId, request);

  @override
  Future<SkillRollbackResult> proposeSkillRollback(
    String sessionId,
    String skillName,
    SkillRollbackRequest request,
  ) => _proposeSkillRollbackImpl(sessionId, skillName, request);

  @override
  Future<ApprovalDetails> getApproval(String sessionId, String approvalId) =>
      _getApprovalImpl(sessionId, approvalId);

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
  Future<ResourcePreview> resolveResource(
    String sessionId,
    String uri, {
    String? selector,
  }) => _resolveResourceImpl(sessionId, uri, selector: selector);

  @override
  Future<List<MikuResourceEntry>> listResources(String sessionId, String uri) =>
      _listResourcesImpl(sessionId, uri);

  @override
  Future<int> assignSessionToProject(String projectId, String sessionId) =>
      _assignSessionToProjectImpl(projectId, sessionId);

  @visibleForTesting
  Future<String?> deviceTokenForTesting() => _deviceToken();
}
