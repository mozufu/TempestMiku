class ApprovalNotificationAction {
  const ApprovalNotificationAction({
    required this.sessionId,
    required this.approvalId,
    required this.decision,
    required this.requiresConfirmation,
    this.dedupeKey,
    this.deliveryId,
    this.eventSeq,
    this.expiresAt,
  });

  final String sessionId;
  final String approvalId;
  final String decision;
  final bool requiresConfirmation;
  final String? dedupeKey;
  final String? deliveryId;
  final int? eventSeq;
  final String? expiresAt;

  static ApprovalNotificationAction fromMap(Map<Object?, Object?> value) {
    return ApprovalNotificationAction(
      sessionId: value['sessionId']?.toString() ?? '',
      approvalId: value['approvalId']?.toString() ?? '',
      decision: value['decision']?.toString() ?? '',
      requiresConfirmation: value['requiresConfirmation'] == true,
      dedupeKey: _optionalNotificationString(value['dedupeKey']),
      deliveryId: _optionalNotificationString(value['deliveryId']),
      eventSeq: _optionalNotificationInt(value['eventSeq']),
      expiresAt: _optionalNotificationString(value['expiresAt']),
    );
  }
}

class NotificationRouteAction {
  const NotificationRouteAction({
    required this.sessionId,
    required this.kind,
    this.approvalId,
    this.dedupeKey,
    this.deliveryId,
    this.eventSeq,
    this.expiresAt,
  });

  final String sessionId;
  final String kind;
  final String? approvalId;
  final String? dedupeKey;
  final String? deliveryId;
  final int? eventSeq;
  final String? expiresAt;

  static NotificationRouteAction fromMap(Map<Object?, Object?> value) {
    return NotificationRouteAction(
      sessionId: value['sessionId']?.toString() ?? '',
      kind: value['routeKind']?.toString() ?? '',
      approvalId: _optionalNotificationString(value['approvalId']),
      dedupeKey: _optionalNotificationString(value['dedupeKey']),
      deliveryId: _optionalNotificationString(value['deliveryId']),
      eventSeq: _optionalNotificationInt(value['eventSeq']),
      expiresAt: _optionalNotificationString(value['expiresAt']),
    );
  }
}

String? _optionalNotificationString(Object? value) {
  final text = value?.toString() ?? '';
  return text.isEmpty ? null : text;
}

int? _optionalNotificationInt(Object? value) {
  if (value is num) return value.toInt();
  return int.tryParse(value?.toString() ?? '');
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

abstract class ActionableNotificationService {
  Stream<NotificationRouteAction> get routes;

  Future<void> configureReplyAuthority({
    String? serverBaseUrl,
    String? deviceToken,
  });

  /// Cancels queued inline replies without prompting or changing permission.
  Future<void> cancelPendingReplies();
}

enum NotificationPermissionStatus { unsupported, granted, denied }

abstract class MikuNotificationService {
  bool get isSupported;

  Stream<ApprovalNotificationAction> get actions;

  Future<void> initialize();

  /// Reads the current OS state without showing a permission prompt.
  Future<NotificationPermissionStatus> permissionStatus();

  /// May show the OS permission prompt and must only follow explicit user input.
  Future<bool> requestPermission();

  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
    required String action,
    String? expiresAt,
  });

  Future<void> cancelApproval(String approvalId);
}
