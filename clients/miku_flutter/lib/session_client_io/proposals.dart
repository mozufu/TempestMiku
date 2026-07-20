part of '../session_client_io.dart';

extension _NativeProposalClient on NativeMikuSessionClient {
  Future<MemoryWriteProposalResult> _proposeMemoryWriteImpl(
    String sessionId,
    MemoryWriteProposalRequest request,
  ) async {
    final json = await _request(
      'POST',
      '/sessions/$sessionId/memory/proposals',
      body: request.toJson(),
    );
    return MemoryWriteProposalResult.fromJson(json);
  }

  Future<EvolutionReviewProposalResult> _proposeEvolutionReviewImpl(
    String sessionId,
    EvolutionReviewProposalRequest request,
  ) async {
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/review-proposals',
      body: request.toJson(),
    );
    return EvolutionReviewProposalResult.fromJson(json);
  }

  Future<ModeAddendumRollbackResult> _proposeModeAddendumRollbackImpl(
    String sessionId,
    String modeId,
    AddendumRollbackRequest request,
  ) async {
    final name = Uri.encodeComponent(modeId);
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/modes/$name/rollback',
      body: request.toJson(),
    );
    return ModeAddendumRollbackResult.fromJson(json);
  }

  Future<PersonaAddendumRollbackResult> _proposePersonaAddendumRollbackImpl(
    String sessionId,
    String personaId,
    AddendumRollbackRequest request,
  ) async {
    final name = Uri.encodeComponent(personaId);
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/personas/$name/rollback',
      body: request.toJson(),
    );
    return PersonaAddendumRollbackResult.fromJson(json);
  }

  Future<SkillRollbackResult> _proposeSkillRollbackImpl(
    String sessionId,
    String skillName,
    SkillRollbackRequest request,
  ) async {
    final name = Uri.encodeComponent(skillName);
    final json = await _request(
      'POST',
      '/sessions/$sessionId/evolution/skills/$name/rollback',
      body: request.toJson(),
    );
    return SkillRollbackResult.fromJson(json);
  }

  Future<ApprovalDetails> _getApprovalImpl(
    String sessionId,
    String approvalId,
  ) async {
    final json = await _request(
      'GET',
      '/sessions/$sessionId/approvals/$approvalId',
    );
    return ApprovalDetails.fromJson(json);
  }
}
