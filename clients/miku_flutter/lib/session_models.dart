import 'dart:async';
import 'dart:math';

import 'package:flutter/foundation.dart';

String newClientMessageId() {
  final random = Random.secure();
  final bytes = List<int>.generate(16, (_) => random.nextInt(256));
  final encoded =
      bytes.map((byte) => byte.toRadixString(16).padLeft(2, '0')).join();
  return 'm_$encoded';
}

/// Retries one ambiguous message transport failure without changing the idempotency key.
Future<void> sendIdempotentMessageWithRetry({
  required String clientMessageId,
  required Future<void> Function(String clientMessageId) send,
  required bool Function(Object error) isAmbiguousFailure,
  int maxAttempts = 2,
  Duration retryDelay = const Duration(milliseconds: 250),
}) async {
  if (maxAttempts < 1) {
    throw ArgumentError.value(maxAttempts, 'maxAttempts', 'must be positive');
  }
  for (var attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      await send(clientMessageId);
      return;
    } catch (error) {
      if (attempt == maxAttempts || !isAmbiguousFailure(error)) rethrow;
      if (retryDelay > Duration.zero) await Future<void>.delayed(retryDelay);
    }
  }
}

class MikuSession {
  const MikuSession({
    required this.id,
    required this.mode,
    required this.label,
    required this.voiceCap,
    this.status = 'active',
    this.defaultScope = 'global',
    this.activeSkills = const [],
    this.lastEventId,
    this.locked = false,
  });

  final String id;
  final String status;
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

class DriveFeed {
  const DriveFeed({
    required this.recent,
    required this.virtualDirs,
    required this.proposals,
    required this.pendingApprovals,
  });

  final List<DriveFeedItem> recent;
  final List<DriveVirtualDir> virtualDirs;
  final List<DriveOrganizerProposal> proposals;
  final List<DrivePendingApproval> pendingApprovals;

  bool get isEmpty =>
      recent.isEmpty &&
      virtualDirs.isEmpty &&
      proposals.isEmpty &&
      pendingApprovals.isEmpty;

  static const empty = DriveFeed(
    recent: [],
    virtualDirs: [],
    proposals: [],
    pendingApprovals: [],
  );

  static DriveFeed fromJson(Map<String, Object?> json) {
    return DriveFeed(
      recent:
          _mapList(json['recent'])
              .map(DriveFeedItem.fromJson)
              .where((item) => item.uri.isNotEmpty)
              .toList(),
      virtualDirs:
          _mapList(json['virtualDirs'] ?? json['virtual_dirs'])
              .map(DriveVirtualDir.fromJson)
              .where((dir) => dir.uri.isNotEmpty)
              .toList(),
      proposals:
          _mapList(json['proposals'])
              .map(DriveOrganizerProposal.fromJson)
              .where((proposal) => proposal.proposalId.isNotEmpty)
              .toList(),
      pendingApprovals:
          _mapList(json['pendingApprovals'] ?? json['pending_approvals'])
              .map(DrivePendingApproval.fromJson)
              .where(
                (approval) =>
                    approval.approvalId.isNotEmpty ||
                    approval.action.isNotEmpty,
              )
              .toList(),
    );
  }
}

class DriveFeedItem {
  const DriveFeedItem({
    required this.uri,
    required this.path,
    this.title,
    this.docKind,
    this.project,
    this.tags = const [],
    this.contentHash,
    this.summary,
    this.snippet,
    this.selector,
    this.sizeBytes,
    this.updatedAt,
  });

  final String uri;
  final String path;
  final String? title;
  final String? docKind;
  final String? project;
  final List<String> tags;
  final String? contentHash;
  final String? summary;
  final String? snippet;
  final String? selector;
  final int? sizeBytes;
  final String? updatedAt;

  String get displayTitle {
    final explicit = title?.trim();
    if (explicit != null && explicit.isNotEmpty) return explicit;
    final leaf = path.split('/').where((part) => part.isNotEmpty).lastOrNull;
    if (leaf != null && leaf.isNotEmpty) return leaf;
    return uri;
  }

  String get displayPreview {
    for (final value in [summary, snippet, path]) {
      final text = value?.trim();
      if (text != null && text.isNotEmpty) return text;
    }
    return uri;
  }

  static DriveFeedItem fromJson(Map<String, Object?> json) {
    return DriveFeedItem(
      uri: _stringValue(json['uri']),
      path: _stringValue(json['path']),
      title: _nullableString(json['title']),
      docKind: _nullableString(json['docKind'] ?? json['doc_kind']),
      project: _nullableString(json['project']),
      tags: _stringList(json['tags']),
      contentHash: _nullableString(json['contentHash'] ?? json['content_hash']),
      summary: _nullableString(json['summary']),
      snippet: _nullableString(json['snippet']),
      selector: _nullableString(json['selector']),
      sizeBytes: _intValue(json['sizeBytes'] ?? json['size_bytes']),
      updatedAt: _nullableString(json['updatedAt'] ?? json['updated_at']),
    );
  }
}

class DriveVirtualDir {
  const DriveVirtualDir({
    required this.uri,
    required this.name,
    required this.kind,
    required this.title,
  });

  final String uri;
  final String name;
  final String kind;
  final String title;

  static DriveVirtualDir fromJson(Map<String, Object?> json) {
    return DriveVirtualDir(
      uri: _stringValue(json['uri']),
      name: _stringValue(json['name']),
      kind: _stringValue(json['kind']),
      title: _stringValue(json['title']),
    );
  }
}

class DriveOrganizerProposal {
  const DriveOrganizerProposal({
    required this.proposalId,
    required this.action,
    required this.status,
    required this.sourcePath,
    this.sourceUri,
    this.proposedPath,
    this.proposedUri,
    this.confidence,
    this.previewTitle,
    this.previewSubtitle,
    this.previewSnippet,
  });

  final String proposalId;
  final String action;
  final String status;
  final String sourcePath;
  final String? sourceUri;
  final String? proposedPath;
  final String? proposedUri;
  final double? confidence;
  final String? previewTitle;
  final String? previewSubtitle;
  final String? previewSnippet;

  String get displayAction =>
      action.isEmpty ? 'organizer proposal' : action.replaceAll('_', ' ');

  String get displayTitle {
    final explicit = previewTitle?.trim();
    if (explicit != null && explicit.isNotEmpty) return explicit;
    return displayAction;
  }

  String get displayPath {
    final proposed = proposedPath?.trim();
    if (proposed != null && proposed.isNotEmpty) {
      return '$sourcePath -> $proposed';
    }
    return sourcePath;
  }

  static DriveOrganizerProposal fromJson(Map<String, Object?> json) {
    final preview = _mapValue(json['preview']);
    return DriveOrganizerProposal(
      proposalId: _stringValue(json['proposalId'] ?? json['id']),
      action: _stringValue(json['action']),
      status:
          _stringValue(json['status']).isEmpty
              ? 'pending'
              : _stringValue(json['status']),
      sourcePath: _stringValue(json['sourcePath'] ?? json['source_path']),
      sourceUri: _nullableString(json['sourceUri'] ?? json['source_uri']),
      proposedPath: _nullableString(
        json['proposedPath'] ?? json['proposed_path'],
      ),
      proposedUri: _nullableString(json['proposedUri'] ?? json['proposed_uri']),
      confidence: _doubleValue(json['confidence']),
      previewTitle: _nullableString(preview?['title']),
      previewSubtitle: _nullableString(preview?['subtitle']),
      previewSnippet: _nullableString(preview?['snippet']),
    );
  }
}

class DrivePendingApproval {
  const DrivePendingApproval({
    required this.approvalId,
    required this.action,
    this.preview,
  });

  final String approvalId;
  final String action;
  final String? preview;

  static DrivePendingApproval fromJson(Map<String, Object?> json) {
    final preview = _mapValue(json['preview']);
    return DrivePendingApproval(
      approvalId: _stringValue(json['approvalId'] ?? json['approval_id']),
      action: _stringValue(json['action']),
      preview: _nullableString(preview?['subtitle'] ?? json['preview']),
    );
  }
}

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
    required this.voiceCap,
    required this.defaultScope,
    required this.capabilityClass,
    required this.activeSkills,
    required this.capabilities,
    this.description = '',
  });

  final String id;
  final String label;
  final String voiceCap;
  final String defaultScope;
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
      voiceCap: _stringValue(json['voiceCap']),
      defaultScope:
          _stringValue(json['defaultScope']).isEmpty
              ? 'global'
              : _stringValue(json['defaultScope']),
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
      status:
          _stringValue(data['status']).isEmpty
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

class EvolutionReviewProposal {
  const EvolutionReviewProposal({
    required this.proposalId,
    required this.targetKind,
    required this.targetId,
    required this.status,
    required this.preview,
    required this.resourceUri,
    required this.applyEnabled,
  });

  final String proposalId;
  final String targetKind;
  final String targetId;
  final String status;
  final String preview;
  final String resourceUri;
  final bool applyEnabled;

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
    );
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
  const ProjectOverview({required this.status, required this.nextActions});

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
    this.content = '',
    this.title,
  });

  final String uri;
  final String kind;
  final String mime;
  final String? title;
  final int sizeBytes;
  final String preview;
  final String content;
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

String normalizeMikuServerBaseUrl(
  String value, {
  bool requireHttps = kReleaseMode,
}) {
  var text = value.trim();
  if (text.isEmpty) {
    throw const FormatException('server target is empty');
  }
  if (!text.contains('://')) {
    text = 'http://$text';
  }
  final uri = Uri.parse(text);
  if (!uri.hasScheme || uri.host.isEmpty) {
    throw const FormatException('server target must include a host');
  }
  if (uri.scheme != 'http' && uri.scheme != 'https') {
    throw const FormatException('server target must use http or https');
  }
  if (requireHttps && uri.scheme != 'https') {
    throw const FormatException(
      'release builds require https for every server target',
    );
  }
  if (uri.userInfo.isNotEmpty) {
    throw const FormatException('server target must not contain credentials');
  }
  if ((uri.path.isNotEmpty && uri.path != '/') ||
      uri.hasQuery ||
      uri.hasFragment) {
    throw const FormatException(
      'server target must be an origin without a path, query, or fragment',
    );
  }
  final normalized =
      uri.replace(path: '', query: null, fragment: null).toString();
  return normalized.endsWith('/')
      ? normalized.substring(0, normalized.length - 1)
      : normalized;
}

class MikuPairingTarget {
  const MikuPairingTarget({required this.serverBaseUrl, required this.code});

  final String serverBaseUrl;
  final String code;

  Uri get serverUri => Uri.parse(serverBaseUrl);

  String get origin => serverUri.origin;

  String get scheme => serverUri.scheme.toUpperCase();

  String get host => serverUri.host;

  int get effectivePort =>
      serverUri.hasPort
          ? serverUri.port
          : serverUri.scheme == 'https'
          ? 443
          : 80;
}

MikuPairingTarget pairingTargetFromLink(String value) {
  final uri = Uri.parse(value.trim());
  if (uri.scheme != 'tempestmiku' || uri.host != 'pair') {
    throw const FormatException('not a TempestMiku pairing link');
  }
  if (uri.queryParameters['v'] != '1') {
    throw const FormatException('unsupported TempestMiku pairing version');
  }
  final server = uri.queryParameters['server']?.trim();
  if (server == null || server.isEmpty) {
    throw const FormatException('pairing link is missing a server target');
  }
  final code = uri.queryParameters['code']?.trim();
  if (code == null || !RegExp(r'^[a-fA-F0-9]{64}$').hasMatch(code)) {
    throw const FormatException('pairing link has an invalid one-time code');
  }
  return MikuPairingTarget(
    serverBaseUrl: normalizeMikuServerBaseUrl(server),
    code: code,
  );
}

abstract class MikuSessionClient {
  Future<ModeCatalog> modeCatalog();

  Future<MikuSession> createOrReuseSession();

  Future<MikuSession> createSession();

  Future<List<SessionSummary>> listSessions({int limit = 30});

  Future<LoadedSession> loadSession(String sessionId);

  Stream<MikuEvent> events(String sessionId, {String? lastEventId});

  void rememberLastEventId(String sessionId, String lastEventId);

  /// Sends one durable user message.
  ///
  /// Callers own [clientMessageId] and must reuse it when retrying an
  /// ambiguously failed send so the server can deduplicate the turn.
  Future<void> sendMessage(
    String sessionId,
    String content, {
    required String clientMessageId,
  });

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

  Future<DriveFeed> driveFeed(
    String sessionId, {
    int limit = 20,
    String? project,
  });

  Future<ResourcePreview> previewResource(String sessionId, String uri);

  Future<ResourcePreview> resolveResource(String sessionId, String uri);

  Future<ProjectPromotion> promoteSession(
    String sessionId, {
    String? summary,
    List<String> openLoops = const [],
    List<String> decisions = const [],
    List<String> resources = const [],
  });
}

abstract class ServerTargetClient {
  String pairingDeviceName();

  Future<String> serverBaseUrl();

  Future<void> setServerBaseUrl(String baseUrl);

  Future<void> pairWithCode(MikuPairingTarget target);

  Future<void> logout();
}

abstract class PushRegistrationClient {
  Future<bool> hasDeviceCredential();

  Future<void> registerPush({
    required String endpoint,
    required String p256dh,
    required String auth,
  });

  Future<void> unregisterPush();
}

Map<String, Object?>? _mapValue(Object? value) {
  if (value is Map<String, Object?>) return value;
  if (value is Map) return value.cast<String, Object?>();
  return null;
}

List<Map<String, Object?>> _mapList(Object? value) {
  return ((value as List?) ?? const [])
      .whereType<Map>()
      .map((item) => item.cast<String, Object?>())
      .toList();
}

List<String> _stringList(Object? value) {
  return ((value as List?) ?? const [])
      .map((item) => item.toString())
      .where((item) => item.isNotEmpty)
      .toList();
}

String _stringValue(Object? value) => value?.toString() ?? '';

String? _nullableString(Object? value) {
  final text = _stringValue(value);
  return text.isEmpty ? null : text;
}

int? _intValue(Object? value) {
  if (value is num) return value.toInt();
  return int.tryParse(_stringValue(value));
}

double? _doubleValue(Object? value) {
  if (value is num) return value.toDouble();
  return double.tryParse(_stringValue(value));
}
