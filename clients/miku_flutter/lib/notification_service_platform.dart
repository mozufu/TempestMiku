abstract class MikuNotificationService {
  bool get isSupported;

  Future<void> initialize();

  Future<bool> requestPermission();

  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
  });

  Future<void> cancelApproval(String approvalId);
}
