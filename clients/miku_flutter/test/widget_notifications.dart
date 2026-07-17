part of 'widget_test.dart';

void _registerNotificationPolicyTests() {
  test('approval notification policy only alerts outside the visible app', () {
    expect(shouldNotifyApproval(AppLifecycleState.resumed), isFalse);
    expect(shouldNotifyApproval(AppLifecycleState.inactive), isTrue);
    expect(shouldNotifyApproval(AppLifecycleState.hidden), isTrue);
    expect(shouldNotifyApproval(AppLifecycleState.paused), isTrue);
    expect(shouldNotifyApproval(AppLifecycleState.detached), isTrue);
  });
}

void _registerNotificationActionTests() {
  testWidgets(
    'background approvals notify and resolved approvals clear alerts',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();
      final notifications = RecordingNotificationService();

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      expect(notifications.initialized, isTrue);

      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pump();
      client.emitEvent(
        session.id,
        const MikuEvent(
          type: 'approval',
          data: {
            'approvalId': 'approval-background',
            'action': 'proc.run cargo clean',
            'scope': {},
            'options': [],
          },
        ),
      );
      await tester.pump();

      expect(notifications.shownApprovals, const [
        'scripted-0:approval-background',
      ]);

      client.emitEvent(
        session.id,
        const MikuEvent(
          type: 'approval_resolved',
          data: {'approvalId': 'approval-background'},
        ),
      );
      await tester.pump();

      expect(notifications.cancelledApprovals, const ['approval-background']);
    },
  );

  testWidgets(
    'notification action loads the target session and approves once',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();
      client.seedPendingApproval(
        session.id,
        approvalId: 'approval-notification-action',
        action: 'proc.run cargo test',
      );
      final notifications = RecordingNotificationService();
      addTearDown(notifications.actionController.close);

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      for (var i = 0; i < 20 && client.driveFeedRequests == 0; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(client.driveFeedRequests, greaterThan(0));
      expect(notifications.actionController.hasListener, isTrue);

      notifications.actionController.add(
        ApprovalNotificationAction(
          sessionId: session.id,
          approvalId: 'approval-notification-action',
          decision: 'approve',
          requiresConfirmation: false,
        ),
      );
      await tester.pump();
      for (var i = 0; i < 10 && client.resolvedApprovals.isEmpty; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }

      expect(
        client.resolvedApprovals,
        contains('approval-notification-action:approve'),
      );
      expect(
        notifications.cancelledApprovals,
        contains('approval-notification-action'),
      );
    },
  );

  testWidgets(
    'notification route restores the exact session without replaying a message',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final target = await client.createSession();
      client.seedPendingApproval(
        target.id,
        approvalId: 'approval-route-target',
        action: 'proc.run cargo test',
      );
      await client.createSession();
      final notifications = ActionableRecordingNotificationService();
      addTearDown(notifications.actionController.close);
      addTearDown(notifications.routeController.close);

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      for (var i = 0; i < 20 && client.driveFeedRequests == 0; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }

      notifications.routeController.add(
        NotificationRouteAction(sessionId: target.id, kind: 'session_ready'),
      );
      for (var i = 0; i < 20; i++) {
        await tester.pump(const Duration(milliseconds: 100));
        if (find
            .text('Pending approval · proc.run cargo test')
            .evaluate()
            .isNotEmpty) {
          break;
        }
      }

      expect(
        find.text('Pending approval · proc.run cargo test'),
        findsOneWidget,
      );
      expect(client.sentClientMessageIds, isEmpty);
    },
  );

  testWidgets('stale notification action syncs and reports expiry', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();
    final notifications = RecordingNotificationService();
    addTearDown(notifications.actionController.close);

    await tester.pumpWidget(
      MikuApp(client: client, notifications: notifications),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    notifications.actionController.add(
      ApprovalNotificationAction(
        sessionId: session.id,
        approvalId: 'approval-already-expired',
        decision: 'deny',
        requiresConfirmation: false,
      ),
    );
    await tester.pump();
    for (var i = 0; i < 10 && notifications.cancelledApprovals.isEmpty; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    await tester.pump();

    expect(client.resolvedApprovals, isEmpty);
    expect(
      notifications.cancelledApprovals,
      contains('approval-already-expired'),
    );
    expect(
      find.text('This approval was already resolved or has expired.'),
      findsOneWidget,
    );
  });

  testWidgets(
    'legacy Android notification action requires in-app confirmation',
    (WidgetTester tester) async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();
      client.seedPendingApproval(
        session.id,
        approvalId: 'approval-legacy-confirm',
        action: 'drive.put inbox/report.md',
        backend: 'drive',
      );
      final notifications = RecordingNotificationService();
      addTearDown(notifications.actionController.close);

      await tester.pumpWidget(
        MikuApp(client: client, notifications: notifications),
      );
      await tester.pump();
      for (var i = 0; i < 20 && client.driveFeedRequests == 0; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      notifications.actionController.add(
        ApprovalNotificationAction(
          sessionId: session.id,
          approvalId: 'approval-legacy-confirm',
          decision: 'deny',
          requiresConfirmation: true,
        ),
      );
      await tester.pump();
      for (
        var i = 0;
        i < 10 && find.byType(AlertDialog).evaluate().isEmpty;
        i++
      ) {
        await tester.pump(const Duration(milliseconds: 100));
      }

      expect(client.resolvedApprovals, isEmpty);
      expect(find.text('drive.put inbox/report.md'), findsWidgets);
      await tester.tap(find.widgetWithText(FilledButton, 'Deny'));
      await tester.pump();
      for (var i = 0; i < 10 && client.resolvedApprovals.isEmpty; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(
        client.resolvedApprovals,
        contains('approval-legacy-confirm:deny'),
      );
    },
  );
}
