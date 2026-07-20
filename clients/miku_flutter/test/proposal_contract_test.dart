import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:miku_flutter/miku_api.dart';
import 'package:miku_flutter/session_client_io.dart';
import 'package:miku_flutter/session_client_stub.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  test('proposal requests serialize the exact bounded Rust wire shapes', () {
    const memory = MemoryWriteProposalRequest.profileFact(
      predicate: 'prefers',
      object: 'reviewable changes',
      confidence: 0.8,
      timeoutMs: 5000,
    );
    const evolution = EvolutionReviewProposalRequest(
      target: EvolutionReviewTarget.persona('miku'),
      changes: [
        EvolutionReviewChange(
          section: 'tone_guidance',
          before: null,
          after: EvolutionReviewMetadata(
            label: 'Tone preference',
            summary: 'Keep routine status updates concise.',
          ),
        ),
      ],
      timeoutMs: 60000,
    );

    expect(memory.toJson(), {
      'memoryKind': 'profile_fact',
      'predicate': 'prefers',
      'object': 'reviewable changes',
      'confidence': 0.8,
      'timeoutMs': 5000,
    });
    expect(evolution.toJson(), {
      'target': {'kind': 'persona', 'personaId': 'miku'},
      'changes': [
        {
          'section': 'tone_guidance',
          'before': null,
          'after': {
            'label': 'Tone preference',
            'summary': 'Keep routine status updates concise.',
          },
        },
      ],
      'timeoutMs': 60000,
    });
    expect(
      const AddendumRollbackRequest(
        expectedActiveDigest: 'sha256:active',
      ).toJson(),
      {'expectedActiveDigest': 'sha256:active', 'targetDigest': null},
    );
    expect(
      const SkillRollbackRequest(
        expectedActiveDigest: 'sha256:active',
        targetDigest: 'sha256:previous',
      ).toJson(),
      {
        'expectedActiveDigest': 'sha256:active',
        'targetDigest': 'sha256:previous',
      },
    );
  });

  test('proposal and approval response models retain exact route fields', () {
    final memory = MemoryWriteProposalResult.fromJson(const {
      'proposalId': 'proposal-1',
      'memoryKind': 'recall_chunk',
      'status': 'approved',
      'record': {
        'id': 'record-1',
        'uri': 'memory://scopes/global/chunks/record-1',
        'kind': 'recall_chunk',
      },
    });
    final evolution = EvolutionReviewProposalResult.fromJson(const {
      'proposalId': 'proposal-2',
      'approvalId': 'approval-2',
      'status': 'pending',
      'resourceUri': 'memory://review-proposals/proposal-2',
      'applyEnabled': true,
    });
    final approval = ApprovalDetails.fromJson(const {
      'approvalId': 'approval-2',
      'sessionId': 'session-1',
      'backend': 'evolution-review',
      'action': 'review persona addendum miku',
      'scope': {
        'kind': 'evolution_review',
        'proposalId': 'proposal-2',
        'timeoutMs': 60000,
      },
      'options': [
        {
          'optionId': 'allow',
          'name': 'Apply persona addendum',
          'kind': 'allow_once',
        },
      ],
      'status': 'pending',
      'createdAt': '2026-07-20T10:00:00Z',
      'expiresAt': '2026-07-20T10:01:00Z',
      'resolvedAt': null,
      'serverTime': '2026-07-20T10:00:05Z',
    });

    expect(memory.record!.kind, 'recall_chunk');
    expect(evolution.approvalId, 'approval-2');
    expect(evolution.applyEnabled, isTrue);
    expect(approval.isPending, isTrue);
    expect(approval.prompt.approvalId, 'approval-2');
    expect(approval.prompt.timeoutMs, 60000);
    expect(approval.options.single.kind, 'allow_once');
  });

  test(
    'scripted memory proposal waits for durable approval resolution',
    () async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();

      final resultFuture = client.proposeMemoryWrite(
        session.id,
        const MemoryWriteProposalRequest.recallChunk(
          text: 'Keep chat as the primary surface.',
          timeoutMs: 5000,
        ),
      );
      final loaded = await client.loadSession(session.id);
      final approvalEvent = loaded.pendingEvents.singleWhere(
        (event) => event.type == 'approval',
      );
      final approvalId = approvalEvent.data['approvalId']! as String;
      final pending = await client.getApproval(session.id, approvalId);

      expect(pending.backend, 'memory');
      expect(pending.status, 'pending');
      await client.resolveApproval(session.id, approvalId, 'approve');
      final result = await resultFuture;
      final resolved = await client.getApproval(session.id, approvalId);

      expect(result.status, 'approved');
      expect(result.record!.kind, 'recall_chunk');
      expect(resolved.status, 'approved');
      expect(resolved.resolvedAt, isNotNull);
    },
  );

  test(
    'scripted evolution and rollback routes return pending approvals',
    () async {
      final client = ScriptedMikuClient();
      final session = await client.createSession();
      final evolution = await client.proposeEvolutionReview(
        session.id,
        const EvolutionReviewProposalRequest(
          target: EvolutionReviewTarget.mode('serious_engineer'),
          changes: [
            EvolutionReviewChange(
              section: 'description',
              after: EvolutionReviewMetadata(
                label: 'Verification',
                summary: 'Lead with evidence.',
              ),
            ),
          ],
        ),
      );
      final modeRollback = await client.proposeModeAddendumRollback(
        session.id,
        'serious_engineer',
        const AddendumRollbackRequest(
          expectedActiveDigest: 'sha256:active',
          targetDigest: 'sha256:previous',
        ),
      );
      final personaRollback = await client.proposePersonaAddendumRollback(
        session.id,
        'miku',
        const AddendumRollbackRequest(expectedActiveDigest: 'sha256:active'),
      );
      final skillRollback = await client.proposeSkillRollback(
        session.id,
        'release-workflow',
        const SkillRollbackRequest(
          expectedActiveDigest: 'sha256:active',
          targetDigest: 'sha256:previous',
        ),
      );

      expect(evolution.status, 'pending');
      expect(evolution.resourceUri, startsWith('memory://review-proposals/'));
      expect(modeRollback.modeId, 'serious_engineer');
      expect(personaRollback.targetDigest, isNull);
      expect(skillRollback.name, 'release-workflow');
      expect(
        (await client.getApproval(
          session.id,
          skillRollback.approvalId,
        )).backend,
        'skill-rollback',
      );
    },
  );

  test('native client uses the exact proposal and approval routes', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    final requests = <({String method, String path, Object? body})>[];
    server.listen((request) async {
      final text = await utf8.decoder.bind(request).join();
      requests.add((
        method: request.method,
        path: request.uri.toString(),
        body: text.isEmpty ? null : jsonDecode(text),
      ));
      final path = request.uri.path;
      final Object response = switch (path) {
        '/sessions/session-1/memory/proposals' => const {
          'proposalId': 'memory-1',
          'memoryKind': 'recall_chunk',
          'status': 'denied',
          'record': null,
        },
        '/sessions/session-1/evolution/review-proposals' => const {
          'proposalId': 'review-1',
          'approvalId': 'approval-review',
          'status': 'pending',
          'resourceUri': 'memory://review-proposals/review-1',
          'applyEnabled': true,
        },
        '/sessions/session-1/evolution/modes/serious_engineer/rollback' =>
          const {
            'approvalId': 'approval-mode',
            'modeId': 'serious_engineer',
            'expectedActiveDigest': 'active-mode',
            'targetDigest': null,
            'status': 'pending',
          },
        '/sessions/session-1/evolution/personas/miku/rollback' => const {
          'approvalId': 'approval-persona',
          'personaId': 'miku',
          'expectedActiveDigest': 'active-persona',
          'targetDigest': 'previous-persona',
          'status': 'pending',
        },
        '/sessions/session-1/evolution/skills/release-workflow/rollback' =>
          const {
            'approvalId': 'approval-skill',
            'name': 'release-workflow',
            'expectedActiveDigest': 'active-skill',
            'targetDigest': 'previous-skill',
            'status': 'pending',
          },
        '/sessions/session-1/approvals/approval-skill' => const {
          'approvalId': 'approval-skill',
          'sessionId': 'session-1',
          'backend': 'skill-rollback',
          'action': 'skill.rollback release-workflow',
          'scope': {'kind': 'skill_rollback', 'timeoutMs': 60000},
          'options': [],
          'status': 'pending',
          'createdAt': '2026-07-20T10:00:00Z',
          'expiresAt': '2026-07-20T10:01:00Z',
          'resolvedAt': null,
          'serverTime': '2026-07-20T10:00:05Z',
        },
        _ => throw StateError('unexpected request $path'),
      };
      request.response
        ..headers.contentType = ContentType.json
        ..write(jsonEncode(response));
      await request.response.close();
    });
    final client = NativeMikuSessionClient(
      tokenStore: MemoryDeviceTokenStore(),
    );
    await client.setServerBaseUrl(
      'http://${server.address.address}:${server.port}',
    );

    await client.proposeMemoryWrite(
      'session-1',
      const MemoryWriteProposalRequest.recallChunk(
        text: 'A bounded note',
        timeoutMs: 5000,
      ),
    );
    await client.proposeEvolutionReview(
      'session-1',
      const EvolutionReviewProposalRequest(
        target: EvolutionReviewTarget.mode('serious_engineer'),
        changes: [
          EvolutionReviewChange(
            section: 'description',
            after: EvolutionReviewMetadata(
              label: 'Evidence',
              summary: 'Lead with verified results.',
            ),
          ),
        ],
      ),
    );
    await client.proposeModeAddendumRollback(
      'session-1',
      'serious_engineer',
      const AddendumRollbackRequest(expectedActiveDigest: 'active-mode'),
    );
    await client.proposePersonaAddendumRollback(
      'session-1',
      'miku',
      const AddendumRollbackRequest(
        expectedActiveDigest: 'active-persona',
        targetDigest: 'previous-persona',
      ),
    );
    await client.proposeSkillRollback(
      'session-1',
      'release-workflow',
      const SkillRollbackRequest(
        expectedActiveDigest: 'active-skill',
        targetDigest: 'previous-skill',
      ),
    );
    await client.getApproval('session-1', 'approval-skill');

    expect(
      requests.map((request) => '${request.method} ${request.path}'),
      const [
        'POST /sessions/session-1/memory/proposals',
        'POST /sessions/session-1/evolution/review-proposals',
        'POST /sessions/session-1/evolution/modes/serious_engineer/rollback',
        'POST /sessions/session-1/evolution/personas/miku/rollback',
        'POST /sessions/session-1/evolution/skills/release-workflow/rollback',
        'GET /sessions/session-1/approvals/approval-skill',
      ],
    );
    expect(requests.first.body, {
      'memoryKind': 'recall_chunk',
      'text': 'A bounded note',
      'timeoutMs': 5000,
    });
    expect(requests[2].body, {
      'expectedActiveDigest': 'active-mode',
      'targetDigest': null,
    });
    expect(requests.last.body, isNull);
  });
}
