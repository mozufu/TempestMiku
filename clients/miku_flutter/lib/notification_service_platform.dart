class ApprovalNotificationAction {
  const ApprovalNotificationAction({
    required this.sessionId,
    required this.approvalId,
    required this.decision,
    required this.requiresConfirmation,
  });

  final String sessionId;
  final String approvalId;
  final String decision;
  final bool requiresConfirmation;
}

class UnifiedPushRegistration {
  const UnifiedPushRegistration({
    required this.endpoint,
    required this.p256dh,
    required this.auth,
  });

  final String endpoint;
  final String p256dh;
  final String auth;
}

enum UnifiedPushEventType { registration, unregistered, registrationFailed }

class UnifiedPushEvent {
  const UnifiedPushEvent({required this.type, this.registration});

  final UnifiedPushEventType type;
  final UnifiedPushRegistration? registration;
}

abstract class UnifiedPushNotificationService {
  Stream<UnifiedPushEvent> get pushEvents;

  Future<UnifiedPushRegistration?> registerUnifiedPush();

  Future<void> unregisterUnifiedPush();
}

abstract class MikuNotificationService {
  bool get isSupported;

  Stream<ApprovalNotificationAction> get actions;

  Future<void> initialize();

  Future<bool> requestPermission();

  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
    required String action,
    String? expiresAt,
  });

  Future<void> cancelApproval(String approvalId);
}
