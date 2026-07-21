part of '../session_models.dart';

String newClientMessageId() {
  final random = Random.secure();
  final bytes = List<int>.generate(16, (_) => random.nextInt(256));
  final encoded =
      bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();
  return 'm_$encoded';
}

/// Retries one ambiguous message transport failure without changing the idempotency key.
Future<T> sendIdempotentMessageWithRetry<T>({
  required String clientMessageId,
  required Future<T> Function(String clientMessageId) send,
  required bool Function(Object error) isAmbiguousFailure,
  int maxAttempts = 2,
  Duration retryDelay = const Duration(milliseconds: 250),
}) async {
  if (maxAttempts < 1) {
    throw ArgumentError.value(maxAttempts, 'maxAttempts', 'must be positive');
  }
  for (var attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await send(clientMessageId);
    } catch (error) {
      if (attempt == maxAttempts || !isAmbiguousFailure(error)) rethrow;
      if (retryDelay > Duration.zero) await Future<void>.delayed(retryDelay);
    }
  }
  throw StateError('message retry loop ended without a result');
}

abstract class MikuSessionClient {
  Future<String> serverBaseUrl();

  Future<ServerReadiness> serverReadiness();

  Future<ServerDiagnostics> serverDiagnostics();

  Future<List<AuthDevice>> authDevices();

  Future<PairingCode> createPairingCode();

  Future<void> revokeAuthDevice(String deviceId);

  Future<void> logout();

  Future<VoiceAsrEngineCatalog> voiceAsrEngines();

  Future<VoiceAsrTranscript> transcribeVoicePcm16({
    required String engineId,
    required String captureId,
    required int sampleRate,
    required Uint8List pcm16,
  });

  Future<void> cancelVoiceAsrTranscription();

  Future<ModeCatalog> modeCatalog();

  Future<MikuSession> createOrReuseSession();

  Future<MikuSession> createSession({String scope = 'global'});

  Future<void> endSession(String sessionId);

  Future<List<SessionSummary>> listSessions({int limit = 30});

  Future<List<ProjectCatalogEntry>> listProjects();

  /// Creates (or returns) a project entity (§30.2). Owner-initiated; idempotent on id.
  Future<ProjectCatalogEntry> createProject(String id, {String? title});

  /// Archives a project entity, tombstoning its memory scope (§30.4).
  Future<ProjectCatalogEntry> archiveProject(
    String projectId, {
    String? reason,
  });

  Future<String> setSessionScope(String sessionId, String scope);

  Future<LoadedSession> loadSession(String sessionId);

  Stream<MikuEvent> events(String sessionId, {String? lastEventId});

  void rememberLastEventId(String sessionId, String lastEventId);

  /// Sends one durable user message.
  ///
  /// Callers own [clientMessageId] and must reuse it when retrying an
  /// ambiguously failed send so the server can deduplicate the turn.
  Future<TurnReceipt> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  });

  Future<SessionTurn> getTurn(String sessionId, String turnId);

  Future<MemoryWriteProposalResult> proposeMemoryWrite(
    String sessionId,
    MemoryWriteProposalRequest request,
  );

  Future<EvolutionReviewProposalResult> proposeEvolutionReview(
    String sessionId,
    EvolutionReviewProposalRequest request,
  );

  Future<ModeAddendumRollbackResult> proposeModeAddendumRollback(
    String sessionId,
    String modeId,
    AddendumRollbackRequest request,
  );

  Future<PersonaAddendumRollbackResult> proposePersonaAddendumRollback(
    String sessionId,
    String personaId,
    AddendumRollbackRequest request,
  );

  Future<SkillRollbackResult> proposeSkillRollback(
    String sessionId,
    String skillName,
    SkillRollbackRequest request,
  );

  Future<ApprovalDetails> getApproval(String sessionId, String approvalId);

  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  });

  Future<void> lockMode(String sessionId, String mode);

  Future<void> unlockMode(String sessionId);

  Future<void> overrideMode(String sessionId, String mode);

  Future<ProjectOverview> projectOverview(String sessionId);

  Future<DriveFeed> driveFeed(
    String sessionId, {
    int limit = 20,
    String? project,
  });

  Future<ResourcePreview> previewResource(String sessionId, String uri);

  Future<ResourcePreview> resolveResource(
    String sessionId,
    String uri, {
    String? selector,
  });

  Future<List<MikuResourceEntry>> listResources(String sessionId, String uri);

  /// Assigns a closed session to a project (§30); returns the number of project items grown by the
  /// server's observation catch-up.
  Future<int> assignSessionToProject(String projectId, String sessionId);
}

abstract class ServerTargetClient {
  String pairingDeviceName();

  Future<String> serverBaseUrl();

  Future<void> setServerBaseUrl(String baseUrl);

  Future<void> pairWithCode(MikuPairingTarget target);

  Future<void> logout();
}

/// Installation-local identity returned by the last successful pairing.
///
/// Callers must still use the authenticated `/auth/devices` response as the
/// source of truth for current server state. A null value means this client
/// cannot safely associate its current credential/cookie with a device row.
abstract class CurrentAuthDeviceClient {
  Future<String?> currentAuthDeviceId();
}

abstract class PushRegistrationClient {
  Future<bool> hasDeviceCredential();

  Future<PushRegistrationMetadata> registerPush({
    required String endpoint,
    required String p256dh,
    required String auth,
  });

  Future<void> unregisterPush();
}

class NotificationReplyAuthority {
  const NotificationReplyAuthority({
    required this.serverBaseUrl,
    required this.deviceToken,
  });

  final String serverBaseUrl;
  final String deviceToken;
}

abstract class NotificationReplyAuthorityClient {
  Future<NotificationReplyAuthority?> notificationReplyAuthority();
}
