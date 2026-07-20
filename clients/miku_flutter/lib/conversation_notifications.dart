import 'dart:async';
import 'dart:collection';

import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'notification_service.dart';
import 'session_models.dart';

typedef NotificationOpenSession = Future<void> Function(String sessionId);
typedef NotificationOpenApproval =
    Future<void> Function(String sessionId, ApprovalDetails approval);
typedef NotificationConfirmLegacyAction =
    Future<bool> Function(
      ApprovalNotificationAction action,
      ApprovalDetails approval,
    );
typedef NotificationQuietNotice = void Function(String message);

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

class _QueuedNotificationWork {
  const _QueuedNotificationWork({required this.key, required this.run});

  final String key;
  final Future<void> Function() run;
}

class _NotificationAuthorityTransition {
  const _NotificationAuthorityTransition({
    required this.preserveIntent,
    required this.localOptIn,
  });

  final bool preserveIntent;
  final bool localOptIn;
}

/// Owns the Android notification bridge without making it part of the chat
/// transport. All notification-originated work is authenticated again through
/// [MikuSessionClient] before it can affect conversation UI or an approval.
class BackgroundNotificationCoordinator extends ChangeNotifier {
  BackgroundNotificationCoordinator({
    required this.client,
    required this.notifications,
    NotificationOpenSession? onOpenSession,
    NotificationOpenApproval? onOpenApproval,
    NotificationConfirmLegacyAction? onConfirmLegacyAction,
    NotificationQuietNotice? onQuietNotice,
  }) : _onOpenSession = onOpenSession ?? _ignoreSession,
       _onOpenApproval = onOpenApproval ?? _ignoreApproval,
       _onConfirmLegacyAction =
           onConfirmLegacyAction ?? _rejectUnconfirmedLegacyAction,
       _onQuietNotice = onQuietNotice ?? _ignoreNotice;

  static const preferenceKey = 'tempestmiku.backgroundNotificationsEnabled';
  static const _maximumRememberedWork = 256;

  final MikuSessionClient client;
  final MikuNotificationService notifications;
  final NotificationOpenSession _onOpenSession;
  final NotificationOpenApproval _onOpenApproval;
  final NotificationConfirmLegacyAction _onConfirmLegacyAction;
  final NotificationQuietNotice _onQuietNotice;

  final Queue<_QueuedNotificationWork> _work = Queue();
  final LinkedHashSet<String> _rememberedWork = LinkedHashSet();
  final Set<String> _shownApprovalIds = <String>{};
  final List<StreamSubscription<Object?>> _subscriptions = [];

  BackgroundNotificationPermission _permission =
      BackgroundNotificationPermission.unknown;
  BackgroundNotificationSyncState _syncState =
      BackgroundNotificationSyncState.loading;
  bool _localOptIn = false;
  bool _busy = false;
  bool _started = false;
  bool _disposed = false;
  bool _initialConnectionComplete = false;
  bool _draining = false;
  int _forcedPushSyncAttempt = 0;
  int _authorityEpoch = 0;
  AppLifecycleState _lifecycle = AppLifecycleState.resumed;
  _NotificationAuthorityTransition? _authorityTransition;

  BackgroundNotificationSnapshot get snapshot => BackgroundNotificationSnapshot(
    localOptIn: _localOptIn,
    permission: _permission,
    syncState: _syncState,
    busy: _busy,
  );

  bool get isSupported =>
      notifications.isSupported &&
      notifications is UnifiedPushNotificationService &&
      notifications is ActionableNotificationService;

  /// Subscribes to native streams before doing any asynchronous initialization.
  /// This never requests OS permission.
  Future<void> initialize() async {
    if (_started || _disposed) return;
    _started = true;
    _subscribeBeforeBootstrap();
    await _bootstrapWithoutPrompt();
  }

  void setInitialConnectionComplete() {
    if (_initialConnectionComplete || _disposed) return;
    _initialConnectionComplete = true;
    _drainWork();
  }

  void setLifecycleState(AppLifecycleState state) {
    _lifecycle = state;
  }

  Future<void> setEnabled(bool enabled) async {
    if (_disposed || _busy || enabled == _localOptIn) return;
    if (enabled) {
      await _enableFromExplicitUserAction();
    } else {
      await _disableLocallyAndRemotely();
    }
  }

  Future<void> retrySync() async {
    if (_disposed || _busy) return;
    if (!_localOptIn) {
      if (_syncState ==
          BackgroundNotificationSyncState.serverCleanupUnconfirmed) {
        await _retryDisabledCleanup();
      }
      return;
    }
    final epoch = _authorityEpoch;
    if (_permission != BackgroundNotificationPermission.granted) {
      await _enableFromExplicitUserAction();
      return;
    }
    _busy = true;
    _notify();
    try {
      await _syncEnabled(epoch: epoch, forceRegistrationSync: true);
    } finally {
      if (!_disposed && epoch == _authorityEpoch) {
        _busy = false;
        _notify();
      }
    }
  }

  /// Clears queued actions and local reply authority before a credential or
  /// server origin can change. Remote cleanup is best-effort and its failure is
  /// retained as a truthful UI state instead of leaking endpoint details.
  Future<bool> prepareAuthorityChange({required bool preserveIntent}) async {
    if (_disposed || _authorityTransition != null) return false;
    final transition = _NotificationAuthorityTransition(
      preserveIntent: preserveIntent,
      localOptIn: _localOptIn,
    );
    _authorityTransition = transition;
    final epoch = ++_authorityEpoch;
    _clearQueuedWork();
    _busy = true;
    _syncState = BackgroundNotificationSyncState.transitioningAuthority;
    _notify();

    var localCleanupSucceeded = true;
    var serverCleanupConfirmed = true;
    try {
      localCleanupSucceeded = await _clearLocalReplyAuthority();
      await _cancelAllShownApprovals();
      final pushClient =
          client is PushRegistrationClient
              ? client as PushRegistrationClient
              : null;
      if (pushClient != null && await pushClient.hasDeviceCredential()) {
        try {
          await pushClient.unregisterPush();
        } catch (_) {
          serverCleanupConfirmed = false;
        }
      }
      final push = _pushService;
      if (push != null) {
        try {
          await push.unregisterUnifiedPush();
        } catch (_) {
          localCleanupSucceeded = false;
        }
      }
    } finally {
      if (!_disposed && epoch == _authorityEpoch) {
        _busy = false;
        if (!localCleanupSucceeded) {
          _syncState = BackgroundNotificationSyncState.distributorUnavailable;
        } else if (!serverCleanupConfirmed) {
          _syncState = BackgroundNotificationSyncState.serverCleanupUnconfirmed;
        }
        _notify();
      }
    }
    if (!localCleanupSucceeded) {
      await abortAuthorityChange();
      return false;
    }
    return true;
  }

  /// Restores the old installation intent after a failed re-pair. The old
  /// endpoint remains opaque and is simply re-registered through its provider.
  Future<void> abortAuthorityChange() async {
    final transition = _authorityTransition;
    if (_disposed || transition == null) return;
    _authorityTransition = null;
    final epoch = ++_authorityEpoch;
    _clearQueuedWork();
    _localOptIn = transition.localOptIn;
    if (!_localOptIn) {
      _syncState = BackgroundNotificationSyncState.off;
      _notify();
      return;
    }
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(preferenceKey, true);
    await _syncEnabled(epoch: epoch, forceRegistrationSync: true);
  }

  /// Rebinds the preserved opt-in to the new credential. When the committed
  /// transition is a logout, intent and local distributor state are cleared.
  Future<void> commitAuthorityChange() async {
    final transition = _authorityTransition;
    if (_disposed || transition == null) return;
    _authorityTransition = null;
    final epoch = ++_authorityEpoch;
    _clearQueuedWork();
    final pushClient =
        client is PushRegistrationClient
            ? client as PushRegistrationClient
            : null;
    final hasCredential =
        pushClient != null && await pushClient.hasDeviceCredential();
    final keepEnabled =
        transition.preserveIntent && transition.localOptIn && hasCredential;
    _localOptIn = keepEnabled;
    if (keepEnabled) {
      final prefs = await SharedPreferences.getInstance();
      await prefs.setBool(preferenceKey, true);
      await _syncEnabled(epoch: epoch, forceRegistrationSync: true);
      return;
    }
    if (transition.localOptIn) {
      final prefs = await SharedPreferences.getInstance();
      await prefs.setBool(preferenceKey, false);
    }
    _syncState = BackgroundNotificationSyncState.off;
    _notify();
  }

  Future<void> showApprovalWhileBackgrounded({
    required String sessionId,
    required ApprovalPrompt approval,
    String? expiresAt,
  }) async {
    if (_disposed ||
        !_localOptIn ||
        _permission != BackgroundNotificationPermission.granted ||
        !shouldNotifyApproval(_lifecycle)) {
      return;
    }
    try {
      await notifications.showApproval(
        sessionId: sessionId,
        approvalId: approval.approvalId,
        action: approval.action,
        expiresAt: expiresAt,
      );
      _shownApprovalIds.add(approval.approvalId);
    } catch (_) {
      // The authenticated in-app card remains the source of truth.
    }
  }

  Future<void> cancelApproval(String approvalId) async {
    if (approvalId.isEmpty) return;
    _shownApprovalIds.remove(approvalId);
    try {
      await notifications.cancelApproval(approvalId);
    } catch (_) {
      // Stale native notifications are non-authoritative.
    }
  }

  void _subscribeBeforeBootstrap() {
    _subscriptions.add(
      notifications.actions.listen(
            _enqueueAction,
            onError: (_) => _setDistributorUnavailable(),
          )
          as StreamSubscription<Object?>,
    );
    final actionable = _actionableService;
    if (actionable != null) {
      _subscriptions.add(
        actionable.routes.listen(
              _enqueueRoute,
              onError: (_) => _setDistributorUnavailable(),
            )
            as StreamSubscription<Object?>,
      );
    }
    final push = _pushService;
    if (push != null) {
      _subscriptions.add(
        push.pushEvents.listen(
              _enqueuePushEvent,
              onError: (_) => _setDistributorUnavailable(),
            )
            as StreamSubscription<Object?>,
      );
    }
  }

  Future<void> _bootstrapWithoutPrompt() async {
    NotificationPermissionStatus? nativePermission;
    try {
      await notifications.initialize();
      nativePermission = await notifications.permissionStatus();
    } catch (_) {
      nativePermission = null;
    }
    _permission = _permissionFromNative(nativePermission);

    final prefs = await SharedPreferences.getInstance();
    final hasSavedIntent = prefs.containsKey(preferenceKey);
    var enabled = prefs.getBool(preferenceKey) ?? false;
    if (!hasSavedIntent) {
      final pushClient =
          client is PushRegistrationClient
              ? client as PushRegistrationClient
              : null;
      final hasCredential =
          pushClient != null && await pushClient.hasDeviceCredential();
      enabled =
          hasCredential &&
          _permission == BackgroundNotificationPermission.granted;
      await prefs.setBool(preferenceKey, enabled);
    }
    if (_disposed) return;
    _localOptIn = enabled;
    if (!enabled) {
      _syncState = BackgroundNotificationSyncState.off;
      _notify();
      return;
    }
    if (_permission == BackgroundNotificationPermission.blocked) {
      _syncState = BackgroundNotificationSyncState.permissionBlocked;
      _notify();
      return;
    }
    if (_permission != BackgroundNotificationPermission.granted) {
      _syncState = BackgroundNotificationSyncState.permissionUnknown;
      _notify();
      return;
    }
    await _syncEnabled(epoch: _authorityEpoch);
  }

  Future<void> _enableFromExplicitUserAction() async {
    if (!isSupported) {
      _permission = BackgroundNotificationPermission.unsupported;
      _syncState = BackgroundNotificationSyncState.distributorUnavailable;
      _notify();
      return;
    }
    _busy = true;
    _localOptIn = true;
    _syncState = BackgroundNotificationSyncState.syncing;
    _notify();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(preferenceKey, true);
    try {
      if (_permission != BackgroundNotificationPermission.granted) {
        final granted = await notifications.requestPermission();
        _permission =
            granted
                ? BackgroundNotificationPermission.granted
                : BackgroundNotificationPermission.blocked;
        if (!granted) {
          _syncState = BackgroundNotificationSyncState.permissionBlocked;
          return;
        }
      }
      await _syncEnabled(epoch: _authorityEpoch);
    } catch (_) {
      _syncState = BackgroundNotificationSyncState.distributorUnavailable;
    } finally {
      if (!_disposed) {
        _busy = false;
        _notify();
      }
    }
  }

  Future<void> _disableLocallyAndRemotely() async {
    final epoch = ++_authorityEpoch;
    _localOptIn = false;
    _busy = true;
    _clearQueuedWork();
    _notify();
    final prefs = await SharedPreferences.getInstance();
    await prefs.setBool(preferenceKey, false);
    var localCleanupSucceeded = await _clearLocalReplyAuthority();
    await _cancelAllShownApprovals();
    var serverCleanupConfirmed = true;
    final pushClient =
        client is PushRegistrationClient
            ? client as PushRegistrationClient
            : null;
    if (pushClient != null && await pushClient.hasDeviceCredential()) {
      try {
        await pushClient.unregisterPush();
      } catch (_) {
        serverCleanupConfirmed = false;
      }
    }
    final push = _pushService;
    if (push != null) {
      try {
        await push.unregisterUnifiedPush();
      } catch (_) {
        localCleanupSucceeded = false;
      }
    }
    if (_disposed || epoch != _authorityEpoch) return;
    _busy = false;
    _syncState =
        !localCleanupSucceeded
            ? BackgroundNotificationSyncState.distributorUnavailable
            : !serverCleanupConfirmed
            ? BackgroundNotificationSyncState.serverCleanupUnconfirmed
            : BackgroundNotificationSyncState.off;
    _notify();
  }

  Future<void> _retryDisabledCleanup() async {
    final epoch = ++_authorityEpoch;
    _busy = true;
    _notify();
    var serverCleanupConfirmed = true;
    var localCleanupSucceeded = await _clearLocalReplyAuthority();
    final pushClient =
        client is PushRegistrationClient
            ? client as PushRegistrationClient
            : null;
    if (pushClient != null && await pushClient.hasDeviceCredential()) {
      try {
        await pushClient.unregisterPush();
      } catch (_) {
        serverCleanupConfirmed = false;
      }
    }
    final push = _pushService;
    if (push != null) {
      try {
        await push.unregisterUnifiedPush();
      } catch (_) {
        localCleanupSucceeded = false;
      }
    }
    if (_disposed || epoch != _authorityEpoch) return;
    _busy = false;
    _syncState =
        !localCleanupSucceeded
            ? BackgroundNotificationSyncState.distributorUnavailable
            : !serverCleanupConfirmed
            ? BackgroundNotificationSyncState.serverCleanupUnconfirmed
            : BackgroundNotificationSyncState.off;
    _notify();
  }

  Future<void> _syncEnabled({
    required int epoch,
    bool forceRegistrationSync = false,
  }) async {
    if (_disposed || epoch != _authorityEpoch || !_localOptIn) return;
    if (_permission != BackgroundNotificationPermission.granted) {
      _syncState = BackgroundNotificationSyncState.permissionBlocked;
      _notify();
      return;
    }
    final authorityClient =
        client is NotificationReplyAuthorityClient
            ? client as NotificationReplyAuthorityClient
            : null;
    final pushClient =
        client is PushRegistrationClient
            ? client as PushRegistrationClient
            : null;
    final actionable = _actionableService;
    final push = _pushService;
    if (authorityClient == null || pushClient == null) {
      _syncState = BackgroundNotificationSyncState.serverUnavailable;
      _notify();
      return;
    }
    if (actionable == null || push == null) {
      _syncState = BackgroundNotificationSyncState.distributorUnavailable;
      _notify();
      return;
    }

    final NotificationReplyAuthority? authority;
    try {
      authority = await authorityClient.notificationReplyAuthority();
    } catch (_) {
      _syncState = BackgroundNotificationSyncState.serverUnavailable;
      _notify();
      return;
    }
    if (authority == null || epoch != _authorityEpoch || !_localOptIn) {
      _syncState = BackgroundNotificationSyncState.serverUnavailable;
      _notify();
      return;
    }
    try {
      await actionable.configureReplyAuthority(
        serverBaseUrl: authority.serverBaseUrl,
        deviceToken: authority.deviceToken,
      );
    } catch (_) {
      _syncState = BackgroundNotificationSyncState.distributorUnavailable;
      _notify();
      return;
    }

    _syncState = BackgroundNotificationSyncState.syncing;
    _notify();
    UnifiedPushRegistration? registration;
    try {
      registration = await push.registerUnifiedPush();
    } catch (_) {
      _syncState = BackgroundNotificationSyncState.distributorUnavailable;
      _notify();
      return;
    }
    if (_disposed || epoch != _authorityEpoch || !_localOptIn) return;
    if (registration == null) {
      _syncState = BackgroundNotificationSyncState.waitingEndpoint;
      _notify();
      return;
    }
    _enqueuePushRegistration(registration, force: forceRegistrationSync);
  }

  Future<bool> _clearLocalReplyAuthority() async {
    final actionable = _actionableService;
    if (actionable == null) return true;
    try {
      await actionable.cancelPendingReplies();
      await actionable.configureReplyAuthority();
      return true;
    } catch (_) {
      return false;
    }
  }

  void _enqueueAction(ApprovalNotificationAction action) {
    final key =
        action.dedupeKey?.trim().isNotEmpty == true
            ? 'action:${action.dedupeKey}'
            : 'action:${action.deliveryId ?? '-'}:${action.sessionId}:'
                '${action.approvalId}:${action.decision}:${action.eventSeq ?? '-'}';
    _enqueueWork(key, () => _handleApprovalAction(action));
  }

  void _enqueueRoute(NotificationRouteAction route) {
    final key =
        route.dedupeKey?.trim().isNotEmpty == true
            ? 'route:${route.dedupeKey}'
            : 'route:${route.deliveryId ?? '-'}:${route.kind}:${route.sessionId}:'
                '${route.approvalId ?? '-'}:${route.eventSeq ?? '-'}';
    _enqueueWork(key, () => _handleRoute(route));
  }

  void _enqueuePushEvent(UnifiedPushEvent event) {
    final registration = event.registration;
    final key =
        'push:${event.type.name}:'
        '${registration == null ? '-' : Object.hash(registration.endpoint, registration.p256dh)}';
    _enqueueWork(key, () => _handlePushEvent(event));
  }

  void _enqueuePushRegistration(
    UnifiedPushRegistration registration, {
    bool force = false,
  }) {
    final baseKey =
        'push:${UnifiedPushEventType.registration.name}:'
        '${Object.hash(registration.endpoint, registration.p256dh)}';
    final key = force ? '$baseKey:retry-${_forcedPushSyncAttempt++}' : baseKey;
    _enqueueWork(
      key,
      () => _syncRegistrationWithServer(registration, _authorityEpoch),
    );
  }

  void _enqueueWork(String key, Future<void> Function() run) {
    if (_disposed || !_rememberedWork.add(key)) return;
    while (_rememberedWork.length > _maximumRememberedWork) {
      _rememberedWork.remove(_rememberedWork.first);
    }
    _work.add(_QueuedNotificationWork(key: key, run: run));
    _drainWork();
  }

  Future<void> _drainWork() async {
    if (_disposed ||
        _draining ||
        !_initialConnectionComplete ||
        _authorityTransition != null) {
      return;
    }
    _draining = true;
    try {
      while (!_disposed &&
          _initialConnectionComplete &&
          _authorityTransition == null &&
          _work.isNotEmpty) {
        final work = _work.removeFirst();
        try {
          await work.run();
        } catch (_) {
          _onQuietNotice('通知動作沒有完成；請回到 App 內重試。');
        }
      }
    } finally {
      _draining = false;
    }
  }

  Future<void> _handlePushEvent(UnifiedPushEvent event) async {
    if (!_localOptIn) return;
    switch (event.type) {
      case UnifiedPushEventType.registration:
        final registration = event.registration;
        if (registration == null) {
          _syncState = BackgroundNotificationSyncState.waitingEndpoint;
          _notify();
          return;
        }
        await _syncRegistrationWithServer(registration, _authorityEpoch);
      case UnifiedPushEventType.unregistered:
        _syncState = BackgroundNotificationSyncState.waitingEndpoint;
        _notify();
      case UnifiedPushEventType.registrationFailed:
        _syncState = BackgroundNotificationSyncState.distributorUnavailable;
        _notify();
    }
  }

  Future<void> _syncRegistrationWithServer(
    UnifiedPushRegistration registration,
    int epoch,
  ) async {
    if (_disposed || epoch != _authorityEpoch || !_localOptIn) return;
    final pushClient =
        client is PushRegistrationClient
            ? client as PushRegistrationClient
            : null;
    if (pushClient == null) {
      _syncState = BackgroundNotificationSyncState.serverUnavailable;
      _notify();
      return;
    }
    _syncState = BackgroundNotificationSyncState.syncing;
    _notify();
    try {
      final receipt = await pushClient.registerPush(
        endpoint: registration.endpoint,
        p256dh: registration.p256dh,
        auth: registration.auth,
      );
      if (_disposed || epoch != _authorityEpoch || !_localOptIn) return;
      _syncState =
          receipt.acknowledgedActive
              ? BackgroundNotificationSyncState.syncedThisLaunch
              : BackgroundNotificationSyncState.serverUnavailable;
    } catch (_) {
      if (_disposed || epoch != _authorityEpoch || !_localOptIn) return;
      _syncState = BackgroundNotificationSyncState.serverUnavailable;
    }
    _notify();
  }

  Future<void> _handleRoute(NotificationRouteAction route) async {
    if (route.sessionId.isEmpty) return;
    if (route.kind == 'session_ready') {
      await _onOpenSession(route.sessionId);
      return;
    }
    final approvalId = route.approvalId ?? '';
    if (route.kind != 'approval_requested' || approvalId.isEmpty) return;
    final approval = await _validatedPendingApproval(
      route.sessionId,
      approvalId,
    );
    if (approval == null) return;
    await _onOpenApproval(route.sessionId, approval);
  }

  Future<void> _handleApprovalAction(ApprovalNotificationAction action) async {
    if (action.sessionId.isEmpty ||
        action.approvalId.isEmpty ||
        (action.decision != 'approve' && action.decision != 'deny')) {
      return;
    }
    final approval = await _validatedPendingApproval(
      action.sessionId,
      action.approvalId,
    );
    if (approval == null) return;
    if (action.requiresConfirmation &&
        !await _onConfirmLegacyAction(action, approval)) {
      return;
    }
    try {
      await client.resolveApproval(
        action.sessionId,
        action.approvalId,
        action.decision,
      );
      await cancelApproval(action.approvalId);
    } catch (_) {
      ApprovalDetails? refreshed;
      try {
        refreshed = await client.getApproval(
          action.sessionId,
          action.approvalId,
        );
      } catch (_) {
        // The quiet failure below remains fail-closed.
      }
      if (refreshed == null || !_isPendingAndFresh(refreshed)) {
        await cancelApproval(action.approvalId);
        _onQuietNotice('這個核准已失效或已處理。');
        return;
      }
      _onQuietNotice('這個決定尚未送出，請在 App 內重試。');
      await _onOpenApproval(action.sessionId, refreshed);
    }
  }

  Future<ApprovalDetails?> _validatedPendingApproval(
    String sessionId,
    String approvalId,
  ) async {
    ApprovalDetails details;
    try {
      details = await client.getApproval(sessionId, approvalId);
    } catch (_) {
      await cancelApproval(approvalId);
      _onQuietNotice('這則核准通知已無法確認。');
      return null;
    }
    if (details.sessionId != sessionId ||
        details.approvalId != approvalId ||
        !_isPendingAndFresh(details)) {
      await cancelApproval(approvalId);
      _onQuietNotice('這個核准已失效或已處理。');
      return null;
    }
    return details;
  }

  bool _isPendingAndFresh(ApprovalDetails details) {
    if (!details.isPending) return false;
    final serverTime = DateTime.tryParse(details.serverTime)?.toUtc();
    final expiresAt = DateTime.tryParse(details.expiresAt)?.toUtc();
    if (serverTime == null || expiresAt == null) return false;
    // Do not substitute the device clock for the server's authoritative clock.
    return serverTime.isBefore(expiresAt);
  }

  Future<void> _cancelAllShownApprovals() async {
    final approvals = _shownApprovalIds.toList(growable: false);
    _shownApprovalIds.clear();
    for (final approvalId in approvals) {
      try {
        await notifications.cancelApproval(approvalId);
      } catch (_) {
        // Continue clearing other non-authoritative notifications.
      }
    }
  }

  void _clearQueuedWork() {
    _work.clear();
    _rememberedWork.clear();
  }

  void _setDistributorUnavailable() {
    if (_disposed || !_localOptIn) return;
    _syncState = BackgroundNotificationSyncState.distributorUnavailable;
    _notify();
  }

  UnifiedPushNotificationService? get _pushService =>
      notifications is UnifiedPushNotificationService
          ? notifications as UnifiedPushNotificationService
          : null;

  ActionableNotificationService? get _actionableService =>
      notifications is ActionableNotificationService
          ? notifications as ActionableNotificationService
          : null;

  BackgroundNotificationPermission _permissionFromNative(
    NotificationPermissionStatus? status,
  ) => switch (status) {
    NotificationPermissionStatus.granted =>
      BackgroundNotificationPermission.granted,
    NotificationPermissionStatus.denied =>
      BackgroundNotificationPermission.blocked,
    NotificationPermissionStatus.unsupported =>
      BackgroundNotificationPermission.unsupported,
    null => BackgroundNotificationPermission.unknown,
  };

  void _notify() {
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _authorityEpoch += 1;
    _clearQueuedWork();
    for (final subscription in _subscriptions) {
      unawaited(subscription.cancel());
    }
    _subscriptions.clear();
    super.dispose();
  }

  static Future<void> _ignoreSession(String _) async {}

  static Future<void> _ignoreApproval(String _, ApprovalDetails __) async {}

  static Future<bool> _rejectUnconfirmedLegacyAction(
    ApprovalNotificationAction _,
    ApprovalDetails __,
  ) async => false;

  static void _ignoreNotice(String _) {}
}

/// Low-frequency settings UI. It intentionally exposes no endpoint, token, or
/// provider secret and labels sync only as evidence observed during this app
/// launch.
class BackgroundNotificationsSettingsPanel extends StatelessWidget {
  const BackgroundNotificationsSettingsPanel({
    required this.coordinator,
    super.key,
  });

  final BackgroundNotificationCoordinator coordinator;

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: coordinator,
      builder: (context, _) {
        final snapshot = coordinator.snapshot;
        final visual = _NotificationStatusVisual.from(snapshot, context);
        return DecoratedBox(
          decoration: BoxDecoration(
            border: Border.all(color: Theme.of(context).dividerColor),
            borderRadius: BorderRadius.circular(14),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              ConstrainedBox(
                constraints: const BoxConstraints(minHeight: 64),
                child: SwitchListTile.adaptive(
                  key: const Key('background-notifications-switch'),
                  value: snapshot.localOptIn,
                  onChanged:
                      snapshot.busy ||
                              snapshot.permission ==
                                  BackgroundNotificationPermission.unsupported
                          ? null
                          : coordinator.setEnabled,
                  title: const Text('背景通知'),
                  subtitle: const Text('只在 App 不在前景時提醒；核准內容仍需回到 App 確認。'),
                  secondary: const Icon(Icons.notifications_none_rounded),
                ),
              ),
              const Divider(height: 1),
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 12, 12, 12),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Icon(visual.icon, color: visual.color, size: 20),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            visual.title,
                            key: const Key('background-notifications-status'),
                            style: Theme.of(context).textTheme.labelLarge,
                          ),
                          const SizedBox(height: 2),
                          Text(
                            visual.detail,
                            style: Theme.of(context).textTheme.bodySmall,
                          ),
                        ],
                      ),
                    ),
                    if (!snapshot.busy &&
                        ((snapshot.localOptIn && !snapshot.syncedThisLaunch) ||
                            snapshot.syncState ==
                                BackgroundNotificationSyncState
                                    .serverCleanupUnconfirmed))
                      SizedBox(
                        height: 44,
                        child: TextButton(
                          key: const Key('retry-background-notifications'),
                          onPressed: coordinator.retrySync,
                          child: const Text('重試'),
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _NotificationStatusVisual {
  const _NotificationStatusVisual({
    required this.icon,
    required this.color,
    required this.title,
    required this.detail,
  });

  final IconData icon;
  final Color color;
  final String title;
  final String detail;

  factory _NotificationStatusVisual.from(
    BackgroundNotificationSnapshot snapshot,
    BuildContext context,
  ) {
    final scheme = Theme.of(context).colorScheme;
    final muted = scheme.onSurfaceVariant;
    return switch (snapshot.syncState) {
      BackgroundNotificationSyncState.loading => _NotificationStatusVisual(
        icon: Icons.hourglass_empty_rounded,
        color: muted,
        title: '正在讀取這台裝置的狀態',
        detail: '不會在啟動時要求通知權限。',
      ),
      BackgroundNotificationSyncState.off => _NotificationStatusVisual(
        icon: Icons.notifications_off_outlined,
        color: muted,
        title: '這台裝置已在本機關閉',
        detail: '目前不會建立新的背景通知註冊。',
      ),
      BackgroundNotificationSyncState.permissionBlocked =>
        _NotificationStatusVisual(
          icon: Icons.block_rounded,
          color: scheme.error,
          title: '系統通知權限未開啟',
          detail: 'TempestMiku 沒有在背景顯示通知的系統權限。',
        ),
      BackgroundNotificationSyncState.permissionUnknown =>
        _NotificationStatusVisual(
          icon: Icons.help_outline_rounded,
          color: muted,
          title: '通知權限狀態未知',
          detail: '這台裝置目前無法讀取系統通知權限。',
        ),
      BackgroundNotificationSyncState.waitingEndpoint =>
        _NotificationStatusVisual(
          icon: Icons.hourglass_top_rounded,
          color: scheme.primary,
          title: '正在等待通知服務提供位址',
          detail: '尚未收到可同步到伺服器的 UnifiedPush 位址。',
        ),
      BackgroundNotificationSyncState.syncing => _NotificationStatusVisual(
        icon: Icons.sync_rounded,
        color: scheme.primary,
        title: '正在同步這台裝置',
        detail: '只有成功收到伺服器回條後才會顯示已同步。',
      ),
      BackgroundNotificationSyncState.syncedThisLaunch =>
        _NotificationStatusVisual(
          icon: Icons.check_circle_outline_rounded,
          color: scheme.primary,
          title: '本次啟動已同步',
          detail: '這表示本次 PUT 已收到回條，不代表日後伺服器狀態。',
        ),
      BackgroundNotificationSyncState.serverUnavailable =>
        _NotificationStatusVisual(
          icon: Icons.cloud_off_outlined,
          color: scheme.error,
          title: '伺服器目前無法同步',
          detail: '這台裝置可能尚未配對，或伺服器暫時無法使用。',
        ),
      BackgroundNotificationSyncState.distributorUnavailable =>
        _NotificationStatusVisual(
          icon: Icons.portable_wifi_off_rounded,
          color: scheme.error,
          title: '裝置通知服務目前無法使用',
          detail: '本機通知提供者沒有回應；未顯示或傳送任何位址內容。',
        ),
      BackgroundNotificationSyncState.serverCleanupUnconfirmed =>
        _NotificationStatusVisual(
          icon: Icons.warning_amber_rounded,
          color: scheme.error,
          title: '已在本機關閉；伺服器清理未確認',
          detail: '這台裝置已停止接收，稍後可在有網路時重試伺服器清理。',
        ),
      BackgroundNotificationSyncState.transitioningAuthority =>
        _NotificationStatusVisual(
          icon: Icons.sync_lock_rounded,
          color: scheme.primary,
          title: '正在切換裝置權限',
          detail: '通知佇列與回覆權限已暫停，等待配對結果。',
        ),
    };
  }
}
