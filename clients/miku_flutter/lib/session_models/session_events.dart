part of '../session_models.dart';

enum MikuMemoryPolicy {
  global,
  project;

  static MikuMemoryPolicy fromJson(Object? value) =>
      value == 'project' ? MikuMemoryPolicy.project : MikuMemoryPolicy.global;

  String toJson() => name;
}

const Object _unsetSessionField = Object();

class MikuSession {
  const MikuSession({
    required this.id,
    required this.mode,
    required this.label,
    this.status = 'active',
    this.projectId,
    this.memoryPolicy = MikuMemoryPolicy.global,
    this.activeSkills = const [],
    this.lastEventId,
    this.locked = false,
  });

  final String id;
  final String status;
  final String mode;
  final String label;
  final String? projectId;
  final MikuMemoryPolicy memoryPolicy;
  final List<String> activeSkills;
  final String? lastEventId;
  final bool locked;

  MikuSession copyWith({
    String? id,
    String? status,
    String? mode,
    String? label,
    Object? projectId = _unsetSessionField,
    MikuMemoryPolicy? memoryPolicy,
    List<String>? activeSkills,
    Object? lastEventId = _unsetSessionField,
    bool? locked,
  }) {
    return MikuSession(
      id: id ?? this.id,
      status: status ?? this.status,
      mode: mode ?? this.mode,
      label: label ?? this.label,
      projectId: identical(projectId, _unsetSessionField)
          ? this.projectId
          : projectId as String?,
      memoryPolicy: memoryPolicy ?? this.memoryPolicy,
      activeSkills: activeSkills ?? this.activeSkills,
      lastEventId: identical(lastEventId, _unsetSessionField)
          ? this.lastEventId
          : lastEventId as String?,
      locked: locked ?? this.locked,
    );
  }
}

class MikuEvent {
  const MikuEvent({
    required this.type,
    required this.data,
    this.id,
    this.turnId,
    this.createdAt,
  });

  final String type;
  final Map<String, Object?> data;
  final String? id;
  final String? turnId;
  final String? createdAt;
}

class SessionSummary {
  const SessionSummary({
    required this.id,
    required this.title,
    required this.preview,
    required this.mode,
    required this.label,
    required this.updatedAt,
    required this.status,
    required this.messageCount,
    this.lastEventId,
  });

  final String id;
  final String title;
  final String preview;
  final String mode;
  final String label;
  final String updatedAt;
  final String status;
  final int messageCount;
  final String? lastEventId;
}

class SessionMessage {
  const SessionMessage({
    required this.seq,
    required this.role,
    required this.content,
    required this.createdAt,
  });

  final int seq;
  final String role;
  final String content;
  final String createdAt;
}

class LoadedSession {
  const LoadedSession({
    required this.session,
    required this.messages,
    required this.pendingEvents,
  });

  final MikuSession session;
  final List<SessionMessage> messages;
  final List<MikuEvent> pendingEvents;
}

bool shouldRememberEventId(String type, Map<String, Object?> data) {
  if (type == 'approval') return false;
  if (type == 'write_proposal') {
    return _stringValue(data['status']) != 'pending';
  }
  return true;
}
