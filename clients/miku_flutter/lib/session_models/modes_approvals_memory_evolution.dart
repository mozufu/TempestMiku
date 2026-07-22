part of '../session_models.dart';

class ModeCatalog {
  const ModeCatalog({required this.defaultMode, required this.modes});

  final String defaultMode;
  final List<ModeProfile> modes;

  ModeProfile? find(String id) {
    for (final mode in modes) {
      if (mode.id == id) return mode;
    }
    return null;
  }

  static ModeCatalog fromJson(Map<String, Object?> json) {
    final modes =
        ((json['modes'] as List?) ?? const [])
            .whereType<Map>()
            .map((item) => ModeProfile.fromJson(item.cast<String, Object?>()))
            .where((mode) => mode.id.isNotEmpty)
            .toList();
    final defaultMode = _stringValue(json['defaultMode']);
    return ModeCatalog(
      defaultMode:
          defaultMode.isEmpty && modes.isNotEmpty
              ? modes.first.id
              : defaultMode,
      modes: modes,
    );
  }
}

class ModeProfile {
  const ModeProfile({
    required this.id,
    required this.label,
    required this.capabilityClass,
    required this.activeSkills,
    required this.capabilities,
    this.description = '',
  });

  final String id;
  final String label;
  final String capabilityClass;
  final List<String> activeSkills;
  final List<String> capabilities;
  final String description;

  bool hasCapability(String capability) {
    return capabilities.any((declared) {
      if (declared == capability) return true;
      if (!declared.endsWith('.*')) return false;
      final prefix = declared.substring(0, declared.length - 2);
      return capability.startsWith('$prefix.');
    });
  }

  static ModeProfile fromJson(Map<String, Object?> json) {
    return ModeProfile(
      id: _stringValue(json['mode']),
      label: _stringValue(json['label']),
      capabilityClass:
          _stringValue(json['capabilityClass']).isEmpty
              ? 'conversation'
              : _stringValue(json['capabilityClass']),
      activeSkills:
          ((json['activeSkills'] as List?) ?? const [])
              .map((skill) => skill.toString())
              .toList(),
      capabilities:
          ((json['capabilities'] as List?) ?? const [])
              .map((capability) => capability.toString())
              .toList(),
      description: _stringValue(json['description']),
    );
  }
}

class ApprovalPrompt {
  const ApprovalPrompt({
    required this.approvalId,
    required this.action,
    required this.scope,
    this.backend = '',
    this.options = const [],
    this.timeoutMs,
  });

  final String approvalId;
  final String action;
  final Map<String, Object?> scope;
  final String backend;
  final List<ApprovalOption> options;
  final int? timeoutMs;

  Map<String, Object?>? get proposalScope => _mapValue(scope['proposal']);

  String? get proposalId {
    final direct = _stringValue(scope['proposalId']);
    if (direct.isNotEmpty) return direct;
    final snake = _stringValue(scope['proposal_id']);
    if (snake.isNotEmpty) return snake;
    final proposal = proposalScope;
    if (proposal == null) return null;
    final nested = _stringValue(proposal['proposalId']);
    if (nested.isNotEmpty) return nested;
    final nestedSnake = _stringValue(proposal['proposal_id']);
    return nestedSnake.isEmpty ? null : nestedSnake;
  }

  bool get isMemoryProposal {
    if (backend == 'memory') return true;
    final proposal = proposalScope;
    return proposal != null && _stringValue(proposal['kind']) == 'memory';
  }

  bool get isEvolutionReview =>
      backend == 'evolution-review' ||
      _stringValue(scope['kind']) == 'evolution_review';
}

class ApprovalOption {
  const ApprovalOption({
    required this.optionId,
    required this.name,
    required this.kind,
  });

  final String optionId;
  final String name;
  final String kind;
}

class MemoryWriteProposal {
  const MemoryWriteProposal({
    required this.proposalId,
    required this.memoryKind,
    required this.status,
    required this.text,
    required this.scope,
    required this.subject,
    required this.provenanceLabel,
    required this.source,
    required this.provenance,
    this.predicate,
    this.object,
    this.confidence,
    this.recordUri,
    this.createdAt,
  });

  final String proposalId;
  final String memoryKind;
  final String status;
  final String text;
  final String scope;
  final String subject;
  final String provenanceLabel;
  final String source;
  final Map<String, Object?> provenance;
  final String? predicate;
  final String? object;
  final double? confidence;
  final String? recordUri;
  final String? createdAt;

  bool get isPending => status == 'pending';

  String get kindLabel => switch (memoryKind) {
    'profile_fact' => 'profile fact',
    'recall_chunk' => 'recall chunk',
    _ => memoryKind.isEmpty ? 'memory' : memoryKind.replaceAll('_', ' '),
  };

  String get displayText {
    if (text.isNotEmpty) return text;
    final parts =
        [
          subject,
          predicate,
          object,
        ].whereType<String>().where((part) => part.isNotEmpty).toList();
    return parts.join(' ');
  }

  String get scopeLabel => scope.isEmpty ? 'scope in full proposal' : scope;

  String get provenanceText {
    if (provenanceLabel.isNotEmpty) return provenanceLabel;
    if (source.isNotEmpty) return source;
    return 'full proposal resource';
  }

  static MemoryWriteProposal? fromEvent(Map<String, Object?> data) {
    if (_stringValue(data['kind']) != 'memory') return null;
    final proposalId = _stringValue(data['proposalId']);
    if (proposalId.isEmpty) return null;
    final record = _mapValue(data['record']);
    return MemoryWriteProposal(
      proposalId: proposalId,
      memoryKind: _stringValue(data['memoryKind']),
      status:
          _stringValue(data['status']).isEmpty
              ? 'pending'
              : _stringValue(data['status']),
      text:
          _stringValue(data['text']).isEmpty
              ? _stringValue(data['preview'])
              : _stringValue(data['text']),
      scope: _stringValue(data['scope']),
      subject: _stringValue(data['subject']),
      provenanceLabel: _stringValue(data['provenanceLabel']),
      source: _stringValue(data['source']),
      provenance: _mapValue(data['provenance']) ?? const {},
      predicate: _nullableString(data['predicate']),
      object: _nullableString(data['object']),
      confidence: _doubleValue(data['confidence']),
      recordUri: _nullableString(record?['uri']),
      createdAt: _nullableString(data['createdAt']),
    );
  }

  static MemoryWriteProposal? fromApproval(ApprovalPrompt approval) {
    if (!approval.isMemoryProposal) return null;
    final proposal = approval.proposalScope;
    if (proposal == null) return null;
    return fromEvent({
      ...proposal,
      'kind': 'memory',
      'status': 'pending',
      'source': _stringValue(proposal['source']),
      'provenance': _mapValue(proposal['provenance']) ?? const {},
    });
  }
}

class EvolutionReviewProposal {
  const EvolutionReviewProposal({
    required this.proposalId,
    required this.targetKind,
    required this.targetId,
    required this.status,
    required this.preview,
    required this.resourceUri,
    required this.applyEnabled,
    this.source = '',
    this.candidateTrigger = '',
    this.evidenceCount,
  });

  final String proposalId;
  final String targetKind;
  final String targetId;
  final String status;
  final String preview;
  final String resourceUri;
  final bool applyEnabled;
  final String source;
  final String candidateTrigger;
  final int? evidenceCount;

  bool get isAutoCandidate => source == 'auto_mode';

  static EvolutionReviewProposal? fromEvent(Map<String, Object?> data) {
    if (_stringValue(data['kind']) != 'evolution_review') return null;
    final proposalId = _stringValue(data['proposalId']);
    final target = _mapValue(data['target']);
    if (proposalId.isEmpty || target == null) return null;
    final targetKind = _stringValue(target['kind']);
    final targetId = switch (targetKind) {
      'persona' => _stringValue(target['personaId'] ?? target['persona_id']),
      'mode' => _stringValue(target['modeId'] ?? target['mode_id']),
      _ => '',
    };
    if (targetId.isEmpty) return null;
    return EvolutionReviewProposal(
      proposalId: proposalId,
      targetKind: targetKind,
      targetId: targetId,
      status: _stringValue(data['status']),
      preview: _stringValue(data['preview']),
      resourceUri: _stringValue(data['uri']),
      applyEnabled: data['applyEnabled'] == true,
      source: _stringValue(data['source']),
      candidateTrigger: _stringValue(data['candidateTrigger']),
      evidenceCount: _intValue(data['evidenceCount']),
    );
  }
}
