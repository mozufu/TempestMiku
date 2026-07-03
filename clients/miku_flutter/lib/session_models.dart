import 'dart:async';

class MikuSession {
  const MikuSession({
    required this.id,
    required this.mode,
    required this.label,
    required this.voiceCap,
    this.defaultScope = 'global',
    this.activeSkills = const [],
    this.lastEventId,
    this.locked = false,
  });

  final String id;
  final String mode;
  final String label;
  final String voiceCap;
  final String defaultScope;
  final List<String> activeSkills;
  final String? lastEventId;
  final bool locked;
}

class MikuEvent {
  const MikuEvent({
    required this.type,
    required this.data,
    this.id,
  });

  final String type;
  final Map<String, Object?> data;
  final String? id;
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
    final parts = [subject, predicate, object]
        .whereType<String>()
        .where((part) => part.isNotEmpty)
        .toList();
    return parts.join(' ');
  }

  String get scopeLabel => scope.isEmpty ? 'global' : scope;

  String get provenanceText {
    if (provenanceLabel.isNotEmpty) return provenanceLabel;
    if (source.isNotEmpty) return source;
    return 'unlabeled source';
  }

  static MemoryWriteProposal? fromEvent(Map<String, Object?> data) {
    if (_stringValue(data['kind']) != 'memory') return null;
    final proposalId = _stringValue(data['proposalId']);
    if (proposalId.isEmpty) return null;
    final record = _mapValue(data['record']);
    return MemoryWriteProposal(
      proposalId: proposalId,
      memoryKind: _stringValue(data['memoryKind']),
      status: _stringValue(data['status']).isEmpty
          ? 'pending'
          : _stringValue(data['status']),
      text: _stringValue(data['text']),
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

bool shouldRememberEventId(String type, Map<String, Object?> data) {
  if (type == 'approval') return false;
  if (type == 'write_proposal') {
    return _stringValue(data['status']) != 'pending';
  }
  return true;
}

class ProjectOverview {
  const ProjectOverview({
    required this.status,
    required this.nextActions,
  });

  final String status;
  final List<String> nextActions;
}

class ResourcePreview {
  const ResourcePreview({
    required this.uri,
    required this.kind,
    required this.mime,
    required this.sizeBytes,
    required this.preview,
    required this.hasMore,
    this.title,
  });

  final String uri;
  final String kind;
  final String mime;
  final String? title;
  final int sizeBytes;
  final String preview;
  final bool hasMore;
}

class ProjectPromotion {
  const ProjectPromotion({
    required this.projectUri,
    required this.promotedCount,
  });

  final String projectUri;
  final int promotedCount;
}

abstract class MikuSessionClient {
  Future<MikuSession> createOrReuseSession();

  Future<MikuSession> createSession();

  Stream<MikuEvent> events(String sessionId, {String? lastEventId});

  void rememberLastEventId(String sessionId, String lastEventId);

  Future<void> sendMessage(String sessionId, String content);

  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision, {
    String? optionId,
  });

  Future<void> lockMode(String sessionId, String mode);

  Future<void> unlockMode(String sessionId);

  Future<void> overrideMode(String sessionId, String mode);

  Future<ProjectOverview> projectOverview(String sessionId);

  Future<ResourcePreview> previewResource(String sessionId, String uri);

  Future<ProjectPromotion> promoteSession(
    String sessionId, {
    String? summary,
    List<String> openLoops = const [],
    List<String> decisions = const [],
    List<String> resources = const [],
  });
}

Map<String, Object?>? _mapValue(Object? value) {
  if (value is Map<String, Object?>) return value;
  if (value is Map) return value.cast<String, Object?>();
  return null;
}

String _stringValue(Object? value) => value?.toString() ?? '';

String? _nullableString(Object? value) {
  final text = _stringValue(value);
  return text.isEmpty ? null : text;
}

double? _doubleValue(Object? value) {
  if (value is num) return value.toDouble();
  return double.tryParse(_stringValue(value));
}
