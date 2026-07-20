import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:miku_flutter/notification_service_io.dart';
import 'package:miku_flutter/notification_service_platform.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel('org.mozufu.tempestmiku/notifications');

  tearDown(() async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test(
    'permission inspector reads state without requesting permission',
    () async {
      final calls = <String>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            calls.add(call.method);
            return switch (call.method) {
              'initialize' => null,
              'notificationPermissionStatus' => 'denied',
              _ => throw PlatformException(code: 'unexpected_method'),
            };
          });
      final service = createAndroidNotificationServiceForTesting();

      expect(
        await service.permissionStatus(),
        NotificationPermissionStatus.denied,
      );
      expect(calls, ['initialize', 'notificationPermissionStatus']);
      expect(calls, isNot(contains('requestPermission')));
    },
  );

  test('notification actions retain optional delivery correlation fields', () {
    final legacy = ApprovalNotificationAction.fromMap(const {
      'sessionId': 'session-legacy',
      'approvalId': 'approval-legacy',
      'decision': 'approve',
      'requiresConfirmation': true,
    });
    expect(legacy.dedupeKey, isNull);
    expect(legacy.deliveryId, isNull);
    expect(legacy.eventSeq, isNull);
    expect(legacy.expiresAt, isNull);

    final action = ApprovalNotificationAction.fromMap(const {
      'sessionId': 'session-1',
      'approvalId': 'approval-1',
      'decision': 'deny',
      'requiresConfirmation': false,
      'dedupeKey': 'decision:delivery-1:deny',
      'deliveryId': 'delivery-1',
      'eventSeq': '42',
      'expiresAt': '2026-07-20T12:00:00Z',
    });
    expect(action.dedupeKey, 'decision:delivery-1:deny');
    expect(action.deliveryId, 'delivery-1');
    expect(action.eventSeq, 42);
    expect(action.expiresAt, '2026-07-20T12:00:00Z');
  });

  test('notification routes retain optional delivery correlation fields', () {
    final legacy = NotificationRouteAction.fromMap(const {
      'sessionId': 'session-legacy',
      'routeKind': 'approval_requested',
      'approvalId': 'approval-legacy',
    });
    expect(legacy.dedupeKey, isNull);
    expect(legacy.deliveryId, isNull);

    final route = NotificationRouteAction.fromMap(const {
      'sessionId': 'session-1',
      'routeKind': 'session_ready',
      'approvalId': '',
      'dedupeKey': 'route:delivery-1',
      'deliveryId': 'delivery-1',
      'eventSeq': 84,
      'expiresAt': '2026-07-20T12:00:00Z',
    });
    expect(route.approvalId, isNull);
    expect(route.dedupeKey, 'route:delivery-1');
    expect(route.deliveryId, 'delivery-1');
    expect(route.eventSeq, 84);
    expect(route.expiresAt, '2026-07-20T12:00:00Z');
  });

  test('clearing reply authority requests pending-work cancellation', () async {
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call);
          return null;
        });
    final service = createAndroidNotificationServiceForTesting();
    final actionable = service as ActionableNotificationService;

    await actionable.configureReplyAuthority();

    expect(calls.map((call) => call.method), [
      'initialize',
      'clearInlineReply',
    ]);
    expect(
      (calls.last.arguments as Map<Object?, Object?>)['cancelPendingReplies'],
      isTrue,
    );
  });
}
