part of '../session_models.dart';

String newClientMessageId() {
  final random = Random.secure();
  final bytes = List<int>.generate(16, (_) => random.nextInt(256));
  final encoded =
      bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();
  return 'm_$encoded';
}

/// Retries one ambiguous message transport failure without changing the idempotency key.
Future<void> sendIdempotentMessageWithRetry({
  required String clientMessageId,
  required Future<void> Function(String clientMessageId) send,
  required bool Function(Object error) isAmbiguousFailure,
  int maxAttempts = 2,
  Duration retryDelay = const Duration(milliseconds: 250),
}) async {
  if (maxAttempts < 1) {
    throw ArgumentError.value(maxAttempts, 'maxAttempts', 'must be positive');
  }
  for (var attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      await send(clientMessageId);
      return;
    } catch (error) {
      if (attempt == maxAttempts || !isAmbiguousFailure(error)) rethrow;
      if (retryDelay > Duration.zero) await Future<void>.delayed(retryDelay);
    }
  }
}

abstract class MikuSessionClient {
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

  Future<MikuSession> createSession();

  Future<List<SessionSummary>> listSessions({int limit = 30});

  Future<List<ProjectCatalogEntry>> listProjects();

  Future<String> setSessionScope(String sessionId, String scope);

  Future<LoadedSession> loadSession(String sessionId);

  Stream<MikuEvent> events(String sessionId, {String? lastEventId});

  void rememberLastEventId(String sessionId, String lastEventId);

  /// Sends one durable user message.
  ///
  /// Callers own [clientMessageId] and must reuse it when retrying an
  /// ambiguously failed send so the server can deduplicate the turn.
  Future<void> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  });

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

  Future<ResourcePreview> resolveResource(String sessionId, String uri);

  Future<List<MikuResourceEntry>> listResources(String sessionId, String uri);

  Future<ProjectPromotion> promoteSession(
    String sessionId, {
    String? summary,
    List<String> openLoops = const [],
    List<String> decisions = const [],
    List<String> resources = const [],
  });
}

abstract class ServerTargetClient {
  String pairingDeviceName();

  Future<String> serverBaseUrl();

  Future<void> setServerBaseUrl(String baseUrl);

  Future<void> pairWithCode(MikuPairingTarget target);

  Future<void> logout();
}

abstract class PushRegistrationClient {
  Future<bool> hasDeviceCredential();

  Future<void> registerPush({
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
