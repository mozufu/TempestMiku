import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  Widget appFor(ScriptedMikuClient client) => TempestMikuApp(
    client: client,
    themeMode: ThemeMode.light,
    now: () => DateTime(2026, 7, 20, 20),
  );

  Future<void> loadApp(WidgetTester tester, ScriptedMikuClient client) async {
    tester.view.physicalSize = const Size(430, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(appFor(client));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 80));
  }

  Future<void> openReviewedChanges(WidgetTester tester) async {
    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    final destination = find.byKey(const Key('drawer-reviewed-changes'));
    await tester.scrollUntilVisible(
      destination,
      120,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('left-conversation-drawer')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    await tester.tap(destination);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('reviewed-changes-sheet')), findsOneWidget);
  }

  Future<void> rejectPendingApproval(WidgetTester tester) async {
    final reject = find.byKey(const Key('approval-option-reject'));
    if (reject.evaluate().isEmpty) return;
    await tester.ensureVisible(reject.first);
    await tester.tap(reject.first);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 80));
  }

  testWidgets('memory proposal is review-only and preserves the composer', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await tester.enterText(
      find.byKey(const Key('conversation-composer')),
      '尚未送出的草稿',
    );

    await openReviewedChanges(tester);
    await tester.tap(find.byKey(const Key('propose-memory-change')));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const Key('memory-predicate')), '偏好介面語言');
    await tester.enterText(find.byKey(const Key('memory-object')), '繁體中文');
    await tester.tap(find.byKey(const Key('submit-memory-proposal')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.byKey(const Key('reviewed-changes-sheet')), findsNothing);
    expect(find.byKey(const Key('memory-proposal-details')), findsOneWidget);
    expect(find.textContaining('偏好介面語言'), findsWidgets);
    final composer = tester.widget<TextField>(
      find.byKey(const Key('conversation-composer')),
    );
    expect(composer.controller!.text, '尚未送出的草稿');
    await rejectPendingApproval(tester);
    expect(client.sentClientMessageIds, isEmpty);
  });

  testWidgets('persona guidance uses the bounded reviewed proposal path', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await openReviewedChanges(tester);
    await tester.tap(find.byKey(const Key('propose-guidance-change')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('evolution-change-label')),
      '狀態更新語氣',
    );
    await tester.enterText(
      find.byKey(const Key('evolution-change-summary')),
      '日常進度先講結果，再補一段必要證據。',
    );
    await tester.tap(find.byKey(const Key('submit-evolution-proposal')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));

    expect(find.byKey(const Key('evolution-proposal-details')), findsOneWidget);
    expect(find.text('Persona · miku'), findsOneWidget);
    expect(find.textContaining('狀態更新語氣'), findsWidgets);
    await rejectPendingApproval(tester);
    expect(client.sentClientMessageIds, isEmpty);
  });

  testWidgets('blocks a second reviewed change while one is pending', (
    tester,
  ) async {
    final client = _DelayedEvolutionClient();
    await loadApp(tester, client);
    await openReviewedChanges(tester);
    await tester.tap(find.byKey(const Key('propose-guidance-change')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('evolution-change-label')),
      '仍在處理的變更',
    );
    await tester.enterText(
      find.byKey(const Key('evolution-change-summary')),
      '等待伺服器回覆。',
    );
    await tester.tap(find.byKey(const Key('submit-evolution-proposal')));
    await tester.pump();
    expect(client.proposalCalls, 1);

    await tester.tap(find.byKey(const Key('open-left-drawer')));
    await tester.pumpAndSettle();
    final destination = find.byKey(const Key('drawer-reviewed-changes'));
    await tester.scrollUntilVisible(
      destination,
      120,
      scrollable:
          find
              .descendant(
                of: find.byKey(const Key('left-conversation-drawer')),
                matching: find.byType(Scrollable),
              )
              .first,
    );
    await tester.tap(destination);
    await tester.pump();

    expect(find.textContaining('上一個變更提案仍在處理中'), findsOneWidget);
    expect(client.proposalCalls, 1);
    client.completeProposal();
    await tester.pump();
  });

  testWidgets('rollback validates and confirms both exact digests', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    await loadApp(tester, client);
    await openReviewedChanges(tester);
    await tester.tap(find.byKey(const Key('propose-version-rollback')));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.byKey(const Key('rollback-expected-digest')),
      'not-a-digest',
    );
    await tester.enterText(
      find.byKey(const Key('rollback-target-digest')),
      'also-not-a-digest',
    );
    await tester.tap(find.byKey(const Key('review-rollback-proposal')));
    await tester.pump();
    expect(find.textContaining('64 位小寫十六進位'), findsNWidgets(2));

    final active = 'sha256:${List.filled(64, 'a').join()}';
    final target = 'sha256:${List.filled(64, 'b').join()}';
    await tester.enterText(
      find.byKey(const Key('rollback-expected-digest')),
      active,
    );
    await tester.enterText(
      find.byKey(const Key('rollback-target-digest')),
      target,
    );
    await tester.tap(find.byKey(const Key('review-rollback-proposal')));
    await tester.pumpAndSettle();
    expect(find.text('核對 rollback 版本'), findsOneWidget);
    expect(find.text(active), findsWidgets);
    expect(find.text(target), findsWidgets);

    await tester.tap(find.byKey(const Key('confirm-rollback-proposal')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(find.byKey(const Key('rollback-proposal-details')), findsOneWidget);
    expect(find.text(active), findsWidgets);
    expect(find.text(target), findsWidgets);
    await rejectPendingApproval(tester);
    expect(client.sentClientMessageIds, isEmpty);
  });
}

class _DelayedEvolutionClient extends ScriptedMikuClient {
  final Completer<EvolutionReviewProposalResult> _proposal = Completer();
  int proposalCalls = 0;

  @override
  Future<EvolutionReviewProposalResult> proposeEvolutionReview(
    String sessionId,
    EvolutionReviewProposalRequest request,
  ) {
    proposalCalls += 1;
    return _proposal.future;
  }

  void completeProposal() {
    if (_proposal.isCompleted) return;
    _proposal.complete(
      const EvolutionReviewProposalResult(
        proposalId: 'proposal-delayed',
        approvalId: 'approval-delayed',
        status: 'pending',
        resourceUri: 'memory://review-proposals/proposal-delayed',
        applyEnabled: true,
      ),
    );
  }
}
