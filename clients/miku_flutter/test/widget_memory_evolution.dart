part of 'widget_test.dart';

void _registerEvolutionReviewTests() {
  test('parses server-owned evolution review apply authority', () {
    final proposal = EvolutionReviewProposal.fromEvent(const {
      'kind': 'evolution_review',
      'proposalId': 'proposal-1',
      'target': {'kind': 'mode', 'modeId': 'serious_engineer'},
      'status': 'approved',
      'preview': 'Review verification guidance.',
      'uri': 'memory://review-proposals/proposal-1',
      'applyEnabled': true,
    });
    expect(proposal, isNotNull);
    expect(proposal!.targetKind, 'mode');
    expect(proposal.targetId, 'serious_engineer');
    expect(proposal.status, 'approved');
    expect(proposal.applyEnabled, isTrue);
    expect(proposal.resourceUri, startsWith('memory://review-proposals/'));
  });

  testWidgets('renders server-owned moderate review lifecycle and approval', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    const sessionId = 'scripted-0';
    client.emitEvent(
      sessionId,
      const MikuEvent(
        type: 'write_proposal',
        id: 'review-1',
        data: {
          'kind': 'evolution_review',
          'proposalId': 'proposal-review',
          'target': {'kind': 'mode', 'modeId': 'serious_engineer'},
          'status': 'pending',
          'preview': 'Prefer replayable verification evidence.',
          'uri': 'memory://review-proposals/proposal-review',
          'applyEnabled': true,
        },
      ),
    );
    client.seedPendingApproval(
      sessionId,
      approvalId: 'approval-review',
      backend: 'evolution-review',
      action: 'review mode addendum serious_engineer',
      scope: const {
        'kind': 'evolution_review',
        'proposalId': 'proposal-review',
        'preview': 'Prefer replayable verification evidence.',
        'uri': 'memory://review-proposals/proposal-review',
        'applyEnabled': true,
      },
    );
    client.emitEvent(
      sessionId,
      const MikuEvent(
        type: 'approval',
        id: 'review-2',
        data: {
          'approvalId': 'approval-review',
          'backend': 'evolution-review',
          'action': 'review mode addendum serious_engineer',
          'scope': {
            'kind': 'evolution_review',
            'proposalId': 'proposal-review',
            'preview': 'Prefer replayable verification evidence.',
            'uri': 'memory://review-proposals/proposal-review',
            'applyEnabled': true,
          },
          'options': [
            {
              'optionId': 'allow',
              'name': 'Apply mode addendum',
              'kind': 'allow_once',
            },
            {
              'optionId': 'reject',
              'name': 'Reject proposal',
              'kind': 'reject_once',
            },
          ],
          'timeoutMs': 60000,
        },
      ),
    );
    await tester.pump();

    await _openActivitySheet(tester);
    expect(
      find.textContaining('mode addendum · serious_engineer · pending'),
      findsWidgets,
    );
    expect(find.textContaining('Apply enabled'), findsOneWidget);
    await _closeActivitySheet(tester);

    final card = find.byKey(
      const ValueKey('approval:review mode addendum serious_engineer'),
    );
    await _scrollChatUntilVisible(tester, card);
    expect(card, findsOneWidget);
    await tester.ensureVisible(card);
    await tester.tap(card);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.textContaining('applyEnabled: true'), findsOneWidget);
    expect(find.text('Apply mode addendum'), findsOneWidget);
    await tester.ensureVisible(find.text('Apply mode addendum'));
    await tester.tap(find.text('Apply mode addendum'));
    await tester.pump();
    expect(client.resolvedApprovals, contains('approval-review:approve'));
  });
}

void _registerMemoryProposalTests() {
  testWidgets('renders and resolves memory write proposals', (
    WidgetTester tester,
  ) async {
    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'remember this for me');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Memory proposal'), findsOneWidget);
    expect(
      find.text('Brian prefers approval-backed memory writes.'),
      findsOneWidget,
    );
    expect(find.text('scope global'), findsOneWidget);
    expect(find.text('provenance scripted chat turn'), findsOneWidget);
    expect(
      find.textContaining('Pending approval · memory.write'),
      findsNothing,
    );

    await tester.tap(find.text('Save memory'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.text('Memory proposal'), findsNothing);
  });

  testWidgets('phone view resolves a dream-origin memory proposal', (
    WidgetTester tester,
  ) async {
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    final client = ScriptedMikuClient();
    await tester.pumpWidget(MikuApp(client: client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 50));

    await tester.enterText(find.byType(EditableText), 'dream captured this');
    await tester.pump();
    await tester.tap(find.byIcon(Icons.send));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(find.text('Memory proposal'), findsOneWidget);
    expect(find.text('provenance post-session-dream'), findsOneWidget);
    await tester.tap(find.text('Save memory'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));

    expect(client.resolvedApprovals, hasLength(1));
    expect(client.resolvedApprovals.single, endsWith(':approve'));
    expect(find.text('Memory proposal'), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
