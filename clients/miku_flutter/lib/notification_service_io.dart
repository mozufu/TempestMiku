import 'dart:io';

import 'package:flutter/services.dart';

import 'notification_service_platform.dart';

const _notifications = MethodChannel(
  'dev.tempestmiku.miku_flutter/notifications',
);
const _notificationActions = EventChannel(
  'dev.tempestmiku.miku_flutter/notification-actions',
);

MikuNotificationService createNotificationService() =>
    _AndroidNotificationService();

class _AndroidNotificationService implements MikuNotificationService {
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
}
