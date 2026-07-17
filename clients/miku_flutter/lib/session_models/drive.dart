part of '../session_models.dart';

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
