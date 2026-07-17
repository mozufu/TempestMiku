part of 'widget_test.dart';

void _registerDriveAndResourceTests() {
  testWidgets('shows and resolves pending drive filing approval', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();
    client.seedPendingApproval(
      session.id,
      approvalId: 'approval-drive',
      action: 'drive.put inbox/approval-drop.md',
      scope: const {
        'capability': 'drive.put',
        'sourceUri': 'drop://browser/approval-drop.md',
      },
    );
    client.seedPendingApproval(
      session.id,
      approvalId: 'approval-drive-deny',
      action: 'drive.put inbox/blocked-drop.md',
      scope: const {
        'capability': 'drive.put',
        'sourceUri': 'drop://browser/blocked-drop.md',
      },
    );

    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    final approvalCard = find.byKey(
      const ValueKey('approval:drive.put inbox/approval-drop.md'),
    );
    expect(approvalCard, findsOneWidget);
    expect(
      find.text('Pending approval · drive.put inbox/approval-drop.md'),
      findsOneWidget,
    );

    await _selectDestination(tester, 'Drive');
    expect(find.text('Pending drive approvals'), findsOneWidget);
    expect(find.text('drive.put inbox/approval-drop.md'), findsOneWidget);
    expect(find.text('drive.put inbox/blocked-drop.md'), findsOneWidget);
    await _selectDestination(tester, 'Chat');

    await _scrollChatUntilVisible(tester, approvalCard);
    await tester.ensureVisible(approvalCard);
    await tester.tap(approvalCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.text('Approval needed'), findsOneWidget);
    await tester.tap(find.text('Approve once'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, contains('approval-drive:approve'));
    expect(approvalCard, findsNothing);

    final denyCard = find.byKey(
      const ValueKey('approval:drive.put inbox/blocked-drop.md'),
    );
    expect(denyCard, findsOneWidget);
    await tester.ensureVisible(denyCard);
    await tester.tap(denyCard);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    await tester.ensureVisible(find.text('Deny'));
    await tester.pump();
    await tester.tap(find.text('Deny'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, contains('approval-drive-deny:deny'));
    expect(denyCard, findsNothing);
  });

  testWidgets('dogfoods drive research feed from remote control UI', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(
      find.byType(EditableText),
      'research drive workspace for p5',
    );
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 150));

    expect(find.text('Drive organizer completed'), findsWidgets);
    expect(
      find.textContaining('drive://projects/tempestmiku/research'),
      findsWidgets,
    );

    await _openActivitySheet(tester);
    expect(find.text('Drive organizer completed'), findsWidgets);
    final activityResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(activityResource, findsWidgets);
    await tester.ensureVisible(activityResource.first);
    await tester.pump();
    await tester.tap(activityResource.first);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);

    await _popRoute(tester);
    await _closeActivitySheet(tester);

    await _selectDestination(tester, 'Drive');

    expect(client.driveFeedRequests, greaterThan(0));
    expect(find.text('Drive'), findsWidgets);
    expect(find.text('Recent documents'), findsWidgets);
    expect(find.text('P5 drive research notes'), findsOneWidget);
    expect(find.text('Organizer proposals'), findsOneWidget);
    expect(
      find.textContaining(
        'inbox/raw-research.md -> projects/tempestmiku/research',
      ),
      findsOneWidget,
    );
    expect(find.text('Virtual folders'), findsOneWidget);

    final row = find.byKey(
      const ValueKey(
        'drive-feed:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    final driveList = find.ancestor(of: row, matching: find.byType(ListView));
    await tester.drag(driveList, const Offset(0, -320));
    await tester.pump();
    await tester.pump();
    await tester.tap(row);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(
      find.textContaining('Local citation corpus is ready.'),
      findsOneWidget,
    );
  });

  testWidgets('opens drive uri surfaced by a runtime cell result', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    final session = await client.createSession();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'start runtime');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'cell_result',
        id: 'cell-result-drive-uri',
        data: {
          'status': 'completed',
          'resultPreview':
              '{"filedUri":"drive://projects/tempestmiku/research/p5-drive-workspace.md"}',
        },
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);
    final resultResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(resultResource, findsOneWidget);
    await tester.tap(resultResource);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);
  });

  testWidgets('renders structured runtime cell failures', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    final session = await client.createSession();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'start failed runtime');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'cell_start',
        id: 'cell-start-failed',
        data: {'cellId': 'cell-1', 'sourcePreview': '[redacted]'},
      ),
    );
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'cell_result',
        id: 'cell-result-failed',
        data: {
          'cellId': 'cell-1',
          'status': 'failed',
          'error': 'CapabilityDeniedError: [redacted]',
        },
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);
    expect(find.text('執行程式'), findsOneWidget);
    expect(find.text('程式失敗'), findsWidgets);
    expect(find.text('[redacted]'), findsWidgets);
    expect(
      find.textContaining('CapabilityDeniedError: [redacted]'),
      findsWidgets,
    );
  });

  testWidgets('opens drive uri surfaced by a direct activity payload', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient(pauseBeforeFinal: true);
    final session = await client.createSession();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'file note');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    client.emitEvent(
      session.id,
      const MikuEvent(
        type: 'drive_put',
        id: 'drive-put-direct-uri',
        data: {
          'action': 'put',
          'uri': 'drive://projects/tempestmiku/research/p5-drive-workspace.md',
          'sourceUri': 'drop://browser/raw-research.md',
          'preview': {
            'title': 'Filed drive document',
            'subtitle': 'projects/tempestmiku/research/p5-drive-workspace.md',
            'snippet': 'Drive document content is ready.',
          },
        },
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    await _openActivitySheet(tester);
    final activityResource = find.byKey(
      const ValueKey(
        'activity-resource:drive://projects/tempestmiku/research/p5-drive-workspace.md',
      ),
    );
    expect(activityResource, findsOneWidget);
    expect(
      find.byKey(
        const ValueKey('activity-resource:drop://browser/raw-research.md'),
      ),
      findsNothing,
    );

    await tester.tap(activityResource);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.text('Scripted drive note'), findsOneWidget);
    expect(find.textContaining('# Scripted drive note'), findsOneWidget);
  });
}
