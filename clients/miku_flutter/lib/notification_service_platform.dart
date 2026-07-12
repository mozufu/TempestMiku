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
