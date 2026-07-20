import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/conversation_notifications.dart';
import 'package:miku_flutter/notification_service.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  setUp(() {
    SharedPreferences.setMockInitialValues({});
  });

  test('boot inspects permission without prompting', () async {
    final client = _NotificationClient();
    final notifications = _FakeNotificationService(
      permission: NotificationPermissionStatus.granted,
    );
    final coordinator = BackgroundNotificationCoordinator(
      client: client,
      notifications: notifications,
    );

    await coordinator.initialize();
    coordinator.setInitialConnectionComplete();
    await _flushAsync();

    expect(notifications.permissionReads, 1);
    expect(notifications.permissionRequests, 0);
    // Missing preference + existing credential + already-granted permission is
    // the one migration path that preserves the prior enabled behavior.
    expect(coordinator.snapshot.localOptIn, isTrue);
    expect(coordinator.snapshot.syncedThisLaunch, isTrue);
    coordinator.dispose();
    await notifications.close();
  });

  test(
    'explicit enable requests permission, gets endpoint, then syncs PUT',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: false,
      });
      final client = _NotificationClient();
      final notifications = _FakeNotificationService(
        permission: NotificationPermissionStatus.denied,
        requestResult: true,
      );
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();

      await coordinator.setEnabled(true);
      await _flushAsync();

      expect(notifications.permissionRequests, 1);
      expect(notifications.configureCalls, hasLength(1));
      expect(notifications.registerCalls, 1);
      expect(client.registeredPush, hasLength(1));
      expect(coordinator.snapshot.syncedThisLaunch, isTrue);
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'denied permission stays truthful and never registers an endpoint',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: false,
      });
      final client = _NotificationClient();
      final notifications = _FakeNotificationService(
        permission: NotificationPermissionStatus.denied,
        requestResult: false,
      );
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();

      await coordinator.setEnabled(true);

      expect(coordinator.snapshot.localOptIn, isTrue);
      expect(
        coordinator.snapshot.syncState,
        BackgroundNotificationSyncState.permissionBlocked,
      );
      expect(notifications.registerCalls, 0);
      expect(client.registeredPush, isEmpty);
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'failed DELETE leaves local off and offers a real cleanup retry',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: true,
      });
      final client = _NotificationClient()..failUnregister = true;
      final notifications = _FakeNotificationService(
        permission: NotificationPermissionStatus.granted,
      );
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();
      await _flushAsync();

      await coordinator.setEnabled(false);

      expect(coordinator.snapshot.localOptIn, isFalse);
      expect(
        coordinator.snapshot.syncState,
        BackgroundNotificationSyncState.serverCleanupUnconfirmed,
      );
      client.failUnregister = false;
      await coordinator.retrySync();
      expect(client.unregisterCalls, 2);
      expect(
        coordinator.snapshot.syncState,
        BackgroundNotificationSyncState.off,
      );
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'background approval is shown while foreground approval is not',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: true,
      });
      final client = _NotificationClient();
      final notifications = _FakeNotificationService(
        permission: NotificationPermissionStatus.granted,
      );
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();
      await _flushAsync();
      final prompt = _pendingApproval().prompt;

      coordinator.setLifecycleState(AppLifecycleState.resumed);
      await coordinator.showApprovalWhileBackgrounded(
        sessionId: 'session-1',
        approval: prompt,
      );
      coordinator.setLifecycleState(AppLifecycleState.paused);
      await coordinator.showApprovalWhileBackgrounded(
        sessionId: 'session-1',
        approval: prompt,
      );

      expect(notifications.shownApprovals, ['approval-1']);
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'session-ready route opens exact session and sends zero messages',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: false,
      });
      final opened = <String>[];
      final client = _NotificationClient();
      final notifications = _FakeNotificationService();
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
        onOpenSession: (sessionId) async => opened.add(sessionId),
      );
      await coordinator.initialize();
      notifications.routesController.add(
        const NotificationRouteAction(
          sessionId: 'session-exact',
          kind: 'session_ready',
          dedupeKey: 'route-1',
        ),
      );
      await _flushAsync();
      expect(opened, isEmpty, reason: 'routes wait for the first connection');

      coordinator.setInitialConnectionComplete();
      await _flushAsync();

      expect(opened, ['session-exact']);
      expect(client.sentClientMessageIds, isEmpty);
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'approval route GETs exact pending approval before surfacing it',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: false,
      });
      final client = _NotificationClient()..approval = _pendingApproval();
      final opened = <ApprovalDetails>[];
      final notifications = _FakeNotificationService();
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
        onOpenApproval: (sessionId, approval) async {
          expect(sessionId, 'session-1');
          opened.add(approval);
        },
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();
      notifications.routesController.add(
        const NotificationRouteAction(
          sessionId: 'session-1',
          kind: 'approval_requested',
          approvalId: 'approval-1',
          dedupeKey: 'approval-route-1',
        ),
      );
      await _flushAsync();

      expect(client.callOrder, ['get:session-1:approval-1']);
      expect(opened.single.approvalId, 'approval-1');
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'action GETs before POST, omits optionId, and dedupes delivery',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: false,
      });
      final client =
          _NotificationClient()
            ..approval = _pendingApproval(
              options: const [
                ApprovalOption(
                  optionId: 'wide',
                  name: '總是允許',
                  kind: 'allow_always',
                ),
                ApprovalOption(
                  optionId: 'once',
                  name: '允許一次',
                  kind: 'allow_once',
                ),
              ],
            );
      final notifications = _FakeNotificationService();
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();
      const action = ApprovalNotificationAction(
        sessionId: 'session-1',
        approvalId: 'approval-1',
        decision: 'approve',
        requiresConfirmation: false,
        dedupeKey: 'decision-1',
      );
      notifications.actionsController
        ..add(action)
        ..add(action);
      await _flushAsync();

      expect(client.callOrder, [
        'get:session-1:approval-1',
        'resolve:session-1:approval-1:approve:-',
      ]);
      coordinator.dispose();
      await notifications.close();
    },
  );

  test('stale action is cancelled quietly without POST', () async {
    SharedPreferences.setMockInitialValues({
      BackgroundNotificationCoordinator.preferenceKey: false,
    });
    final notices = <String>[];
    final client =
        _NotificationClient()
          ..approval = _pendingApproval(
            serverTime: '2026-07-20T10:02:00Z',
            expiresAt: '2026-07-20T10:01:00Z',
          );
    final notifications = _FakeNotificationService();
    final coordinator = BackgroundNotificationCoordinator(
      client: client,
      notifications: notifications,
      onQuietNotice: notices.add,
    );
    await coordinator.initialize();
    coordinator.setInitialConnectionComplete();
    notifications.actionsController.add(
      const ApprovalNotificationAction(
        sessionId: 'session-1',
        approvalId: 'approval-1',
        decision: 'deny',
        requiresConfirmation: false,
        dedupeKey: 'stale-1',
      ),
    );
    await _flushAsync();

    expect(client.callOrder, ['get:session-1:approval-1']);
    expect(notifications.cancelledApprovals, ['approval-1']);
    expect(notices.single, contains('失效'));
    coordinator.dispose();
    await notifications.close();
  });

  test(
    'resolve failure GETs again and surfaces only when still pending',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: false,
      });
      final surfaced = <ApprovalDetails>[];
      final client =
          _NotificationClient()
            ..approval = _pendingApproval()
            ..failResolve = true;
      final notifications = _FakeNotificationService();
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
        onOpenApproval: (_, approval) async => surfaced.add(approval),
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();
      notifications.actionsController.add(
        const ApprovalNotificationAction(
          sessionId: 'session-1',
          approvalId: 'approval-1',
          decision: 'deny',
          requiresConfirmation: false,
          dedupeKey: 'retry-get-1',
        ),
      );
      await _flushAsync();

      expect(client.callOrder, [
        'get:session-1:approval-1',
        'resolve:session-1:approval-1:deny:-',
        'get:session-1:approval-1',
      ]);
      expect(surfaced, hasLength(1));
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'failed re-pair preparation can restore old intent and re-register',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: true,
      });
      final client = _NotificationClient();
      final notifications = _FakeNotificationService(
        permission: NotificationPermissionStatus.granted,
      );
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();
      await _flushAsync();
      final registrationsBefore = notifications.registerCalls;

      expect(
        await coordinator.prepareAuthorityChange(preserveIntent: true),
        isTrue,
      );
      await coordinator.abortAuthorityChange();
      await _flushAsync();

      expect(coordinator.snapshot.localOptIn, isTrue);
      expect(notifications.registerCalls, registrationsBefore + 1);
      expect(
        notifications.configureCalls.last.serverBaseUrl,
        'https://miku.example',
      );
      coordinator.dispose();
      await notifications.close();
    },
  );

  test(
    'explicit retry bypasses push receipt dedupe after failed PUT',
    () async {
      SharedPreferences.setMockInitialValues({
        BackgroundNotificationCoordinator.preferenceKey: true,
      });
      final client = _NotificationClient()..failRegister = true;
      final notifications = _FakeNotificationService(
        permission: NotificationPermissionStatus.granted,
      );
      final coordinator = BackgroundNotificationCoordinator(
        client: client,
        notifications: notifications,
      );
      await coordinator.initialize();
      coordinator.setInitialConnectionComplete();
      await _flushAsync();
      expect(client.registerCalls, 1);
      expect(
        coordinator.snapshot.syncState,
        BackgroundNotificationSyncState.serverUnavailable,
      );

      client.failRegister = false;
      await coordinator.retrySync();
      await _flushAsync();

      expect(client.registerCalls, 2);
      expect(coordinator.snapshot.syncedThisLaunch, isTrue);
      coordinator.dispose();
      await notifications.close();
    },
  );

  testWidgets('settings panel exposes large switch and launch-local status', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({
      BackgroundNotificationCoordinator.preferenceKey: true,
    });
    final client = _NotificationClient();
    final notifications = _FakeNotificationService(
      permission: NotificationPermissionStatus.granted,
    );
    final coordinator = BackgroundNotificationCoordinator(
      client: client,
      notifications: notifications,
    );
    await coordinator.initialize();
    coordinator.setInitialConnectionComplete();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 1));
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: BackgroundNotificationsSettingsPanel(coordinator: coordinator),
        ),
      ),
    );
    await tester.pump();

    expect(find.text('本次啟動已同步'), findsOneWidget);
    expect(
      tester
          .getSize(find.byKey(const Key('background-notifications-switch')))
          .height,
      greaterThanOrEqualTo(56),
    );
    expect(find.textContaining('https://push.example'), findsNothing);
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump();
    coordinator.dispose();
  });
}

Future<void> _flushAsync() async {
  for (var i = 0; i < 8; i += 1) {
    await Future<void>.delayed(Duration.zero);
  }
}

ApprovalDetails _pendingApproval({
  List<ApprovalOption> options = const [
    ApprovalOption(optionId: 'allow-once', name: '允許一次', kind: 'allow_once'),
    ApprovalOption(optionId: 'deny-once', name: '拒絕', kind: 'deny_once'),
  ],
  String serverTime = '2026-07-20T10:00:00Z',
  String expiresAt = '2026-07-20T10:01:00Z',
}) => ApprovalDetails(
  approvalId: 'approval-1',
  sessionId: 'session-1',
  backend: 'tm-lang',
  action: '執行受控工作',
  scope: const {'capability': 'proc.run'},
  options: options,
  status: 'pending',
  createdAt: '2026-07-20T09:59:00Z',
  expiresAt: expiresAt,
  serverTime: serverTime,
);

class _NotificationClient extends ScriptedMikuClient
    implements PushRegistrationClient, NotificationReplyAuthorityClient {
  bool credential = true;
  bool failRegister = false;
  bool failUnregister = false;
  bool failResolve = false;
  int registerCalls = 0;
  int unregisterCalls = 0;
  ApprovalDetails? approval;
  final List<UnifiedPushRegistration> registeredPush = [];
  final List<String> callOrder = [];

  @override
  Future<bool> hasDeviceCredential() async => credential;

  @override
  Future<NotificationReplyAuthority?> notificationReplyAuthority() async =>
      credential
          ? const NotificationReplyAuthority(
            serverBaseUrl: 'https://miku.example',
            deviceToken: 'device-token-secret',
          )
          : null;

  @override
  Future<PushRegistrationMetadata> registerPush({
    required String endpoint,
    required String p256dh,
    required String auth,
  }) async {
    registerCalls += 1;
    if (failRegister) throw StateError('server unavailable');
    registeredPush.add(
      UnifiedPushRegistration(endpoint: endpoint, p256dh: p256dh, auth: auth),
    );
    return const PushRegistrationMetadata(
      deviceId: 'device-1',
      provider: 'unifiedpush',
      createdAt: '2026-07-20T10:00:00Z',
      updatedAt: '2026-07-20T10:00:01Z',
    );
  }

  @override
  Future<void> unregisterPush() async {
    unregisterCalls += 1;
    if (failUnregister) throw StateError('server unavailable');
  }

  @override
  Future<ApprovalDetails> getApproval(
    String sessionId,
    String approvalId,
  ) async {
    callOrder.add('get:$sessionId:$approvalId');
    final current = approval;
    if (current == null ||
        current.sessionId != sessionId ||
        current.approvalId != approvalId) {
      throw StateError('missing approval');
    }
    return current;
  }

  @override
  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  }) async {
    callOrder.add(
      'resolve:$sessionId:$approvalId:$decision:${optionId ?? '-'}',
    );
    if (failResolve) throw StateError('ambiguous failure');
  }
}

class _ConfiguredAuthority {
  const _ConfiguredAuthority(this.serverBaseUrl, this.deviceToken);

  final String? serverBaseUrl;
  final String? deviceToken;
}

class _FakeNotificationService
    implements
        MikuNotificationService,
        UnifiedPushNotificationService,
        ActionableNotificationService {
  _FakeNotificationService({
    this.permission = NotificationPermissionStatus.denied,
    this.requestResult = false,
  });

  NotificationPermissionStatus permission;
  bool requestResult;
  int permissionReads = 0;
  int permissionRequests = 0;
  int registerCalls = 0;
  int unregisterCalls = 0;
  bool failUnregister = false;
  final List<_ConfiguredAuthority> configureCalls = [];
  final List<String> shownApprovals = [];
  final List<String> cancelledApprovals = [];
  final actionsController =
      StreamController<ApprovalNotificationAction>.broadcast();
  final routesController =
      StreamController<NotificationRouteAction>.broadcast();
  final pushController = StreamController<UnifiedPushEvent>.broadcast();

  @override
  bool get isSupported => true;

  @override
  Stream<ApprovalNotificationAction> get actions => actionsController.stream;

  @override
  Stream<NotificationRouteAction> get routes => routesController.stream;

  @override
  Stream<UnifiedPushEvent> get pushEvents => pushController.stream;

  @override
  Future<void> initialize() async {}

  @override
  Future<NotificationPermissionStatus> permissionStatus() async {
    permissionReads += 1;
    return permission;
  }

  @override
  Future<bool> requestPermission() async {
    permissionRequests += 1;
    if (requestResult) permission = NotificationPermissionStatus.granted;
    return requestResult;
  }

  @override
  Future<UnifiedPushRegistration?> registerUnifiedPush() async {
    registerCalls += 1;
    return const UnifiedPushRegistration(
      endpoint: 'https://push.example.test/opaque-endpoint',
      p256dh: 'p256dh-secret',
      auth: 'auth-secret',
    );
  }

  @override
  Future<void> unregisterUnifiedPush() async {
    unregisterCalls += 1;
    if (failUnregister) throw StateError('distributor unavailable');
  }

  @override
  Future<void> configureReplyAuthority({
    String? serverBaseUrl,
    String? deviceToken,
  }) async {
    configureCalls.add(_ConfiguredAuthority(serverBaseUrl, deviceToken));
  }

  @override
  Future<void> cancelPendingReplies() async {}

  @override
  Future<void> showApproval({
    required String sessionId,
    required String approvalId,
    required String action,
    String? expiresAt,
  }) async {
    shownApprovals.add(approvalId);
  }

  @override
  Future<void> cancelApproval(String approvalId) async {
    cancelledApprovals.add(approvalId);
  }

  Future<void> close() async {
    await Future.wait<void>([
      actionsController.close(),
      routesController.close(),
      pushController.close(),
    ]);
  }
}
