import 'notification_service_platform.dart';

MikuNotificationService createNotificationService() =>
    _NoopNotificationService();

class _NoopNotificationService implements MikuNotificationService {
  @override
  bool get isSupported => false;

  @override
  Future<void> cancelApproval(String approvalId) async {}

  @override
  Future<void> initialize() async {}

  @override
  Future<bool> requestPermission() async => false;

  @override
  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
  }) async {}
}
