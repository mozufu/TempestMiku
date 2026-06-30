import 'dart:async';

class MikuSession {
  const MikuSession({
    required this.id,
    required this.mode,
    required this.label,
    required this.voiceCap,
  });

  final String id;
  final String mode;
  final String label;
  final String voiceCap;
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
  });

  final String approvalId;
  final String action;
  final Map<String, Object?> scope;
}

class ProjectOverview {
  const ProjectOverview({
    required this.status,
    required this.nextActions,
  });

  final String status;
  final List<String> nextActions;
}

abstract class MikuSessionClient {
  Future<MikuSession> createSession();

  Stream<MikuEvent> events(String sessionId, {String? lastEventId});

  Future<void> sendMessage(String sessionId, String content);

  Future<void> resolveApproval(
    String sessionId,
    String approvalId,
    String decision,
  );

  Future<void> lockMode(String sessionId, String mode);

  Future<void> unlockMode(String sessionId);

  Future<ProjectOverview> projectOverview(String sessionId);
}
