import 'dart:io';

import 'package:flutter/services.dart';

import 'notification_service_platform.dart';

const _notifications = MethodChannel(
  'dev.tempestmiku.miku_flutter/notifications',
);

MikuNotificationService createNotificationService() =>
    _AndroidNotificationService();

class _AndroidNotificationService implements MikuNotificationService {
  bool _initialized = false;

  @override
  bool get isSupported => Platform.isAndroid;

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
  }) async {
    if (!isSupported || approvalId.isEmpty) return;
    await initialize();
    await _notifications.invokeMethod<void>('showApproval', {
      'sessionId': sessionId,
      'approvalId': approvalId,
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
