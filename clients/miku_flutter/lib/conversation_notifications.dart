import 'dart:async';
import 'dart:collection';

import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'design/tm_tokens.dart';
import 'notification_service.dart';
import 'session_models.dart';

part 'conversation_notification_coordinator.dart';
part 'conversation_notification_panel.dart';

typedef NotificationOpenSession = Future<void> Function(String sessionId);
typedef NotificationOpenApproval =
    Future<void> Function(String sessionId, ApprovalDetails approval);
typedef NotificationConfirmLegacyAction =
    Future<bool> Function(
      ApprovalNotificationAction action,
      ApprovalDetails approval,
    );
typedef NotificationQuietNotice = void Function(String message);
typedef ApprovalInFlightCheck = bool Function(String approvalId);

enum BackgroundNotificationPermission { unknown, unsupported, granted, blocked }

enum BackgroundNotificationSyncState {
  loading,
  off,
  permissionBlocked,
  permissionUnknown,
  waitingEndpoint,
  syncing,
  syncedThisLaunch,
  serverUnavailable,
  distributorUnavailable,
  serverCleanupUnconfirmed,
  transitioningAuthority,
}

/// A truthful, installation-local view of background-notification state.
///
/// [syncedThisLaunch] only describes a successful PUT receipt observed by this
/// process. It deliberately does not claim that the server still has a live
/// registration later.
class BackgroundNotificationSnapshot {
  const BackgroundNotificationSnapshot({
    required this.localOptIn,
    required this.permission,
    required this.syncState,
    required this.busy,
  });

  final bool localOptIn;
  final BackgroundNotificationPermission permission;
  final BackgroundNotificationSyncState syncState;
  final bool busy;

  bool get syncedThisLaunch =>
      syncState == BackgroundNotificationSyncState.syncedThisLaunch;
}
