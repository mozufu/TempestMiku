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
    this.options = const [],
    this.timeoutMs,
  });

  final String approvalId;
  final String action;
  final Map<String, Object?> scope;
  final List<ApprovalOption> options;
  final int? timeoutMs;
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
