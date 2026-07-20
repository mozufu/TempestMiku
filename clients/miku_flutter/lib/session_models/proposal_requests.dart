part of '../session_models.dart';

/// Exact request body for `POST /sessions/:id/memory/proposals`.
class MemoryWriteProposalRequest {
  const MemoryWriteProposalRequest({
    required this.memoryKind,
    this.text,
    this.predicate,
    this.object,
    this.confidence,
    this.source,
    this.provenanceLabel,
    this.provenance,
    this.timeoutMs,
  });

  const MemoryWriteProposalRequest.profileFact({
    required String predicate,
    required String object,
    double? confidence,
    String? source,
    String? provenanceLabel,
    Map<String, Object?>? provenance,
    int? timeoutMs,
  }) : this(
         memoryKind: 'profile_fact',
         predicate: predicate,
         object: object,
         confidence: confidence,
         source: source,
         provenanceLabel: provenanceLabel,
         provenance: provenance,
         timeoutMs: timeoutMs,
       );

  const MemoryWriteProposalRequest.recallChunk({
    required String text,
    String? source,
    String? provenanceLabel,
    Map<String, Object?>? provenance,
    int? timeoutMs,
  }) : this(
         memoryKind: 'recall_chunk',
         text: text,
         source: source,
         provenanceLabel: provenanceLabel,
         provenance: provenance,
         timeoutMs: timeoutMs,
       );

  final String memoryKind;
  final String? text;
  final String? predicate;
  final String? object;
  final double? confidence;
  final String? source;
  final String? provenanceLabel;
  final Map<String, Object?>? provenance;
  final int? timeoutMs;

  Map<String, Object?> toJson() => {
    'memoryKind': memoryKind,
    if (text != null) 'text': text,
    if (predicate != null) 'predicate': predicate,
    if (object != null) 'object': object,
    if (confidence != null) 'confidence': confidence,
    if (source != null) 'source': source,
    if (provenanceLabel != null) 'provenanceLabel': provenanceLabel,
    if (provenance != null) 'provenance': provenance,
    if (timeoutMs != null) 'timeoutMs': timeoutMs,
  };
}

class MemoryRecordReference {
  const MemoryRecordReference({
    required this.id,
    required this.uri,
    required this.kind,
  });

  final String id;
  final String uri;
  final String kind;

  static MemoryRecordReference fromJson(Map<String, Object?> json) {
    return MemoryRecordReference(
      id: _stringValue(json['id']),
      uri: _stringValue(json['uri']),
      kind: _stringValue(json['kind']),
    );
  }
}

/// Terminal result returned after the manual memory approval is resolved.
class MemoryWriteProposalResult {
  const MemoryWriteProposalResult({
    required this.proposalId,
    required this.memoryKind,
    required this.status,
    this.record,
  });

  final String proposalId;
  final String memoryKind;
  final String status;
  final MemoryRecordReference? record;

  bool get approved => status == 'approved';

  static MemoryWriteProposalResult fromJson(Map<String, Object?> json) {
    final record = _mapValue(json['record']);
    return MemoryWriteProposalResult(
      proposalId: _stringValue(json['proposalId']),
      memoryKind: _stringValue(json['memoryKind']),
      status: _stringValue(json['status']),
      record: record == null ? null : MemoryRecordReference.fromJson(record),
    );
  }
}

class EvolutionReviewTarget {
  const EvolutionReviewTarget._({required this.kind, required this.id});

  const EvolutionReviewTarget.persona(String personaId)
    : this._(kind: 'persona', id: personaId);

  const EvolutionReviewTarget.mode(String modeId)
    : this._(kind: 'mode', id: modeId);

  final String kind;
  final String id;

  Map<String, Object?> toJson() => switch (kind) {
    'persona' => {'kind': kind, 'personaId': id},
    'mode' => {'kind': kind, 'modeId': id},
    _ => throw StateError('unsupported evolution review target $kind'),
  };
}

class EvolutionReviewMetadata {
  const EvolutionReviewMetadata({required this.label, required this.summary});

  final String label;
  final String summary;

  Map<String, Object?> toJson() => {'label': label, 'summary': summary};
}

class EvolutionReviewChange {
  const EvolutionReviewChange({
    required this.section,
    required this.after,
    this.before,
  });

  final String section;
  final EvolutionReviewMetadata? before;
  final EvolutionReviewMetadata after;

  Map<String, Object?> toJson() => {
    'section': section,
    'before': before?.toJson(),
    'after': after.toJson(),
  };
}

/// Exact bounded addendum review request; it cannot carry a raw patch.
class EvolutionReviewProposalRequest {
  const EvolutionReviewProposalRequest({
    required this.target,
    required this.changes,
    this.timeoutMs,
  });

  final EvolutionReviewTarget target;
  final List<EvolutionReviewChange> changes;
  final int? timeoutMs;

  Map<String, Object?> toJson() => {
    'target': target.toJson(),
    'changes': changes.map((change) => change.toJson()).toList(),
    if (timeoutMs != null) 'timeoutMs': timeoutMs,
  };
}

class EvolutionReviewProposalResult {
  const EvolutionReviewProposalResult({
    required this.proposalId,
    required this.approvalId,
    required this.status,
    required this.resourceUri,
    required this.applyEnabled,
  });

  final String proposalId;
  final String approvalId;
  final String status;
  final String resourceUri;
  final bool applyEnabled;

  static EvolutionReviewProposalResult fromJson(Map<String, Object?> json) {
    return EvolutionReviewProposalResult(
      proposalId: _stringValue(json['proposalId']),
      approvalId: _stringValue(json['approvalId']),
      status: _stringValue(json['status']),
      resourceUri: _stringValue(json['resourceUri']),
      applyEnabled: json['applyEnabled'] == true,
    );
  }
}

/// Request shared by managed mode and persona addendum rollback routes.
/// A null [targetDigest] means deactivate the current addendum.
class AddendumRollbackRequest {
  const AddendumRollbackRequest({
    required this.expectedActiveDigest,
    this.targetDigest,
    this.timeoutMs,
  });

  final String expectedActiveDigest;
  final String? targetDigest;
  final int? timeoutMs;

  Map<String, Object?> toJson() => {
    'expectedActiveDigest': expectedActiveDigest,
    'targetDigest': targetDigest,
    if (timeoutMs != null) 'timeoutMs': timeoutMs,
  };
}

class SkillRollbackRequest {
  const SkillRollbackRequest({
    required this.expectedActiveDigest,
    required this.targetDigest,
    this.timeoutMs,
  });

  final String expectedActiveDigest;
  final String targetDigest;
  final int? timeoutMs;

  Map<String, Object?> toJson() => {
    'expectedActiveDigest': expectedActiveDigest,
    'targetDigest': targetDigest,
    if (timeoutMs != null) 'timeoutMs': timeoutMs,
  };
}

class ModeAddendumRollbackResult {
  const ModeAddendumRollbackResult({
    required this.approvalId,
    required this.modeId,
    required this.expectedActiveDigest,
    required this.targetDigest,
    required this.status,
  });

  final String approvalId;
  final String modeId;
  final String expectedActiveDigest;
  final String? targetDigest;
  final String status;

  static ModeAddendumRollbackResult fromJson(Map<String, Object?> json) {
    return ModeAddendumRollbackResult(
      approvalId: _stringValue(json['approvalId']),
      modeId: _stringValue(json['modeId']),
      expectedActiveDigest: _stringValue(json['expectedActiveDigest']),
      targetDigest: _nullableString(json['targetDigest']),
      status: _stringValue(json['status']),
    );
  }
}

class PersonaAddendumRollbackResult {
  const PersonaAddendumRollbackResult({
    required this.approvalId,
    required this.personaId,
    required this.expectedActiveDigest,
    required this.targetDigest,
    required this.status,
  });

  final String approvalId;
  final String personaId;
  final String expectedActiveDigest;
  final String? targetDigest;
  final String status;

  static PersonaAddendumRollbackResult fromJson(Map<String, Object?> json) {
    return PersonaAddendumRollbackResult(
      approvalId: _stringValue(json['approvalId']),
      personaId: _stringValue(json['personaId']),
      expectedActiveDigest: _stringValue(json['expectedActiveDigest']),
      targetDigest: _nullableString(json['targetDigest']),
      status: _stringValue(json['status']),
    );
  }
}

class SkillRollbackResult {
  const SkillRollbackResult({
    required this.approvalId,
    required this.name,
    required this.expectedActiveDigest,
    required this.targetDigest,
    required this.status,
  });

  final String approvalId;
  final String name;
  final String expectedActiveDigest;
  final String targetDigest;
  final String status;

  static SkillRollbackResult fromJson(Map<String, Object?> json) {
    return SkillRollbackResult(
      approvalId: _stringValue(json['approvalId']),
      name: _stringValue(json['name']),
      expectedActiveDigest: _stringValue(json['expectedActiveDigest']),
      targetDigest: _stringValue(json['targetDigest']),
      status: _stringValue(json['status']),
    );
  }
}

/// Redacted approval snapshot returned by the notification/deep-link route.
class ApprovalDetails {
  const ApprovalDetails({
    required this.approvalId,
    required this.sessionId,
    required this.backend,
    required this.action,
    required this.scope,
    required this.options,
    required this.status,
    required this.createdAt,
    required this.expiresAt,
    required this.serverTime,
    this.resolvedAt,
  });

  final String approvalId;
  final String sessionId;
  final String backend;
  final String action;
  final Map<String, Object?> scope;
  final List<ApprovalOption> options;
  final String status;
  final String createdAt;
  final String expiresAt;
  final String? resolvedAt;
  final String serverTime;

  bool get isPending => status == 'pending';
  bool get isTerminal => !isPending;

  ApprovalPrompt get prompt => ApprovalPrompt(
    approvalId: approvalId,
    action: action,
    scope: scope,
    backend: backend,
    options: options,
    timeoutMs: _intValue(scope['timeoutMs']),
  );

  static ApprovalDetails fromJson(Map<String, Object?> json) {
    final options =
        ((json['options'] as List?) ?? const [])
            .whereType<Map>()
            .map((item) => item.cast<String, Object?>())
            .map(
              (item) => ApprovalOption(
                optionId: _stringValue(item['optionId']),
                name: _stringValue(item['name']),
                kind: _stringValue(item['kind']),
              ),
            )
            .toList();
    return ApprovalDetails(
      approvalId: _stringValue(json['approvalId']),
      sessionId: _stringValue(json['sessionId']),
      backend: _stringValue(json['backend']),
      action: _stringValue(json['action']),
      scope: _mapValue(json['scope']) ?? const {},
      options: options,
      status: _stringValue(json['status']),
      createdAt: _stringValue(json['createdAt']),
      expiresAt: _stringValue(json['expiresAt']),
      resolvedAt: _nullableString(json['resolvedAt']),
      serverTime: _stringValue(json['serverTime']),
    );
  }
}
