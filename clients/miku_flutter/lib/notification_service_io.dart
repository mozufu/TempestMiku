import 'dart:io';

import 'package:flutter/services.dart';

import 'notification_service_platform.dart';

const _notifications = MethodChannel('org.mozufu.tempestmiku/notifications');
const _notificationActions = EventChannel(
  'org.mozufu.tempestmiku/notification-actions',
);
const _unifiedPushEvents = EventChannel(
  'org.mozufu.tempestmiku/unified-push-events',
);

MikuNotificationService createNotificationService() =>
    _AndroidNotificationService();

class _AndroidNotificationService
    implements MikuNotificationService, UnifiedPushNotificationService {
  bool _initialized = false;

  @override
  bool get isSupported => Platform.isAndroid;

  @override
  Stream<ApprovalNotificationAction> get actions {
    if (!isSupported) return const Stream.empty();
    return _notificationActions
        .receiveBroadcastStream()
        .map((raw) {
          final value = (raw as Map).cast<Object?, Object?>();
          return ApprovalNotificationAction(
            sessionId: value['sessionId']?.toString() ?? '',
            approvalId: value['approvalId']?.toString() ?? '',
            decision: value['decision']?.toString() ?? '',
            requiresConfirmation: value['requiresConfirmation'] == true,
          );
        })
        .where(
          (action) =>
              action.sessionId.isNotEmpty &&
              action.approvalId.isNotEmpty &&
              (action.decision == 'approve' || action.decision == 'deny'),
        );
  }

  @override
  Stream<UnifiedPushEvent> get pushEvents {
    if (!isSupported) return const Stream.empty();
    return _unifiedPushEvents.receiveBroadcastStream().map(_pushEventFromRaw);
  }

  @override
  Future<UnifiedPushRegistration?> registerUnifiedPush() async {
    if (!isSupported) return null;
    await initialize();
    final raw = await _notifications.invokeMapMethod<Object?, Object?>(
      'registerUnifiedPush',
    );
    return _registrationFromMap(raw);
  }

  @override
  Future<void> unregisterUnifiedPush() async {
    if (!isSupported) return;
    await initialize();
    await _notifications.invokeMethod<void>('unregisterUnifiedPush');
  }

  @override
  Future<void> initialize() async {
    if (!isSupported || _initialized) return;
    await _notifications.invokeMethod<void>('initialize');
    _initialized = true;
  }

  @override
  Future<bool> requestPermission() async {
    if (!isSupported) return false;
    await initialize();
    return await _notifications.invokeMethod<bool>('requestPermission') ??
        false;
  }

  @override
  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
    required String action,
    String? expiresAt,
  }) async {
    if (!isSupported || approvalId.isEmpty) return;
    await initialize();
    await _notifications.invokeMethod<void>('showApproval', {
      'sessionId': sessionId,
      'approvalId': approvalId,
      'action': action,
      if (expiresAt != null) 'expiresAt': expiresAt,
    });
  }

  @override
  Future<void> cancelApproval(String approvalId) async {
    if (!isSupported || approvalId.isEmpty) return;
    await initialize();
    await _notifications.invokeMethod<void>('cancelApproval', {
      'approvalId': approvalId,
    });
  }

  UnifiedPushEvent _pushEventFromRaw(Object? raw) {
    final value = (raw as Map).cast<Object?, Object?>();
    final type = switch (value['type']?.toString()) {
      'registration' => UnifiedPushEventType.registration,
      'unregistered' => UnifiedPushEventType.unregistered,
      _ => UnifiedPushEventType.registrationFailed,
    };
    return UnifiedPushEvent(
      type: type,
      registration: _registrationFromMap(value['registration']),
    );
  }

  UnifiedPushRegistration? _registrationFromMap(Object? raw) {
    if (raw is! Map) return null;
    final value = raw.cast<Object?, Object?>();
    final endpoint = value['endpoint']?.toString() ?? '';
    final p256dh = value['p256dh']?.toString() ?? '';
    final auth = value['auth']?.toString() ?? '';
    if (endpoint.isEmpty || p256dh.isEmpty || auth.isEmpty) return null;
    return UnifiedPushRegistration(
      endpoint: endpoint,
      p256dh: p256dh,
      auth: auth,
    );
  }
}
