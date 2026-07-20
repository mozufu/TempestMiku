import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:miku_flutter/conversation_app.dart';
import 'package:miku_flutter/session_client_stub.dart';
import 'package:miku_flutter/session_models.dart';

void main() {
  Future<MikuSession> loadApp(
    WidgetTester tester,
    ScriptedMikuClient client,
  ) async {
    final session = await client.createSession();
    await tester.pumpWidget(
      TempestMikuApp(client: client, themeMode: ThemeMode.light),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 80));
    return session;
  }

  Future<void> emit(
    WidgetTester tester,
    ScriptedMikuClient client,
    String sessionId,
    MikuEvent event,
  ) async {
    client.emitEvent(sessionId, event);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 220));
  }

  testWidgets('interleaved effects complete only their correlated activity', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await loadApp(tester, client);

    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'effect_start',
        id: 'effect-a-start',
        data: {'nodeId': 'effect-a'},
      ),
    );
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'effect_start',
        id: 'effect-b-start',
        data: {'nodeId': 'effect-b'},
      ),
    );
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'effect_result',
        id: 'effect-a-result',
        data: {'nodeId': 'effect-a', 'summary': 'A 已完成'},
      ),
    );

    final a = find.byKey(const Key('activity-node:effect-a'));
    final b = find.byKey(const Key('activity-node:effect-b'));
    expect(a, findsOneWidget);
    expect(b, findsOneWidget);
    expect(
      find.descendant(of: a, matching: find.byIcon(Icons.check_rounded)),
      findsOneWidget,
    );
    expect(
      find.descendant(of: a, matching: find.text('A 已完成')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: b, matching: find.byType(CircularProgressIndicator)),
      findsOneWidget,
    );
  });

  testWidgets('keyed effect pause and resume leave sibling activity running', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await loadApp(tester, client);
    for (final node in const ['effect-a', 'effect-b']) {
      await emit(
        tester,
        client,
        session.id,
        MikuEvent(
          type: 'effect_start',
          id: '$node-start',
          data: {'nodeId': node},
        ),
      );
    }

    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'effect_suspended',
        id: 'effect-a-paused',
        data: {'nodeId': 'effect-a'},
      ),
    );
    final a = find.byKey(const Key('activity-node:effect-a'));
    final b = find.byKey(const Key('activity-node:effect-b'));
    expect(find.descendant(of: a, matching: find.text('等待確認')), findsOneWidget);
    expect(
      find.descendant(of: b, matching: find.byType(CircularProgressIndicator)),
      findsOneWidget,
    );

    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'effect_resumed',
        id: 'effect-a-resumed',
        data: {'nodeId': 'effect-a'},
      ),
    );
    expect(find.descendant(of: a, matching: find.text('繼續執行')), findsOneWidget);
    expect(
      find.descendant(of: a, matching: find.byType(CircularProgressIndicator)),
      findsOneWidget,
    );
  });

  testWidgets('runtime and MCP terminal failures never render as success', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await loadApp(tester, client);

    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'cell_start',
        id: 'cell-failed-start',
        data: {'cellId': 'cell-failed'},
      ),
    );
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'cell_result',
        id: 'cell-failed-result',
        data: {
          'cellId': 'cell-failed',
          'status': 'failed',
          'error': '[redacted]',
        },
      ),
    );
    final failedCell = find.byKey(const Key('activity-cell:cell-failed'));
    expect(
      find.descendant(
        of: failedCell,
        matching: find.byIcon(Icons.error_outline_rounded),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: failedCell, matching: find.text('安全工作環境執行未完成')),
      findsOneWidget,
    );

    const mcpBase = {
      'server': 'docs',
      'objectKind': 'resource',
      'objectName': 'lookup',
      'targetDigest': 'sha256:target',
      'requestDigest': 'sha256:request',
    };
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'mcp_invocation',
        id: 'mcp-requested',
        data: {...mcpBase, 'status': 'requested'},
      ),
    );
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'mcp_invocation',
        id: 'mcp-denied',
        data: {...mcpBase, 'status': 'denied'},
      ),
    );
    final deniedMcp = find.byKey(
      const Key(
        'activity-mcp:docs:resource:lookup:sha256:target:sha256:request',
      ),
    );
    expect(
      find.descendant(
        of: deniedMcp,
        matching: find.byIcon(Icons.block_rounded),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: deniedMcp, matching: find.text('外部資源查詢已拒絕或取消')),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: deniedMcp,
        matching: find.byIcon(Icons.check_rounded),
      ),
      findsNothing,
    );
  });

  testWidgets('proposal status updates one compact row by proposal id', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await loadApp(tester, client);
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'write_proposal',
        id: 'proposal-pending',
        data: {
          'proposalId': 'proposal-1',
          'kind': 'memory',
          'status': 'pending',
        },
      ),
    );
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'write_proposal',
        id: 'proposal-approved',
        data: {
          'proposalId': 'proposal-1',
          'kind': 'memory',
          'status': 'approved',
        },
      ),
    );

    expect(find.byKey(const Key('proposal-proposal-1')), findsOneWidget);
    expect(find.text('變更提案已核准'), findsOneWidget);
    expect(find.text('記憶變更'), findsOneWidget);
  });

  testWidgets('actor artifact and history buttons preview their exact URI', (
    tester,
  ) async {
    final client = _PreviewTrackingClient();
    final session = await loadApp(tester, client);
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'actor_spawned',
        id: 'actor-start',
        data: {'actor_id': 'Worker0', 'task': 'bounded task'},
      ),
    );
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'actor_completed',
        id: 'actor-complete',
        data: {
          'actor_id': 'Worker0',
          'summary': 'child completed',
          'artifact_uri': 'artifact://actor-report',
          'history_uri': 'history://Worker0',
        },
      ),
    );
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'actor_resources_linked',
        id: 'actor-links',
        data: {
          'actor_id': 'Worker0',
          'artifact_uri': 'artifact://actor-report',
          'history_uri': 'history://Worker0',
        },
      ),
    );

    final artifact = find.byKey(
      const Key('activity-resource-artifact-artifact://actor-report'),
    );
    final history = find.byKey(
      const Key('activity-resource-history-history://Worker0'),
    );
    expect(artifact, findsOneWidget);
    expect(history, findsOneWidget);
    expect(tester.getSize(artifact).height, greaterThanOrEqualTo(44));
    expect(tester.getSize(history).height, greaterThanOrEqualTo(44));

    await tester.tap(artifact);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(client.previewedUris, ['artifact://actor-report']);
    expect(find.byKey(const Key('event-resource-preview')), findsOneWidget);
    expect(find.text('Preview for artifact://actor-report'), findsOneWidget);
    await tester.tap(find.byTooltip('關閉資源預覽'));
    await tester.pump(const Duration(milliseconds: 350));

    await tester.tap(history);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 350));
    expect(client.previewedUris, [
      'artifact://actor-report',
      'history://Worker0',
    ]);
  });

  testWidgets('unknown events stay quiet and display payloads are bounded', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await loadApp(tester, client);
    await emit(
      tester,
      client,
      session.id,
      const MikuEvent(
        type: 'future_unknown_event',
        id: 'unknown',
        data: {'raw': 'TOPSECRET-DO-NOT-RENDER'},
      ),
    );
    expect(find.textContaining('TOPSECRET'), findsNothing);
    expect(find.byKey(const Key('activity-event:unknown')), findsNothing);

    final oversized = List.filled(4000, 'Z').join();
    await emit(
      tester,
      client,
      session.id,
      MikuEvent(
        type: 'display',
        id: 'large-display',
        data: {'value': oversized},
      ),
    );
    final row = find.byKey(const Key('activity-display:event:large-display'));
    expect(row, findsOneWidget);
    final rendered =
        tester
            .widgetList<Text>(
              find.descendant(of: row, matching: find.byType(Text)),
            )
            .map((widget) => widget.data ?? '')
            .where((text) => text.contains('ZZZZ'))
            .single;
    expect(rendered.length, lessThanOrEqualTo(241));
    expect(rendered, isNot(contains(List.filled(500, 'Z').join())));
  });

  testWidgets('rollback approvals expose bounded target and digest context', (
    tester,
  ) async {
    final client = ScriptedMikuClient();
    final session = await client.createSession();
    client.seedPendingApproval(
      session.id,
      approvalId: 'rollback-review',
      action: 'mode.addendum.rollback general',
      backend: 'evolution-review',
      scope: const {
        'kind': 'mode_addendum_rollback',
        'modeId': 'general',
        'expectedActiveDigest': 'sha256:active',
        'targetDigest': null,
      },
    );
    await tester.pumpWidget(
      TempestMikuApp(client: client, themeMode: ThemeMode.light),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 80));

    expect(find.byKey(const Key('rollback-proposal-details')), findsOneWidget);
    expect(find.text('Mode guidance · general'), findsOneWidget);
    expect(find.text('sha256:active'), findsOneWidget);
    expect(find.text('base（停用 addendum）'), findsOneWidget);
  });
}

final class _PreviewTrackingClient extends ScriptedMikuClient {
  final List<String> previewedUris = [];

  @override
  Future<ResourcePreview> previewResource(String sessionId, String uri) async {
    previewedUris.add(uri);
    return ResourcePreview(
      uri: uri,
      kind: 'text',
      mime: 'text/plain',
      sizeBytes: 64,
      preview: 'Preview for $uri',
      hasMore: false,
    );
  }
}
