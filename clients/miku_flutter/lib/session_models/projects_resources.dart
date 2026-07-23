part of '../session_models.dart';

class ProjectCatalogEntry {
  const ProjectCatalogEntry({
    required this.id,
    required this.title,
    required this.status,
    required this.memoryScope,
    required this.defaultMemoryPolicy,
    required this.projectUri,
    required this.linkedFoldersUri,
    this.linkedFolderUris = const [],
    this.poolId,
  });

  factory ProjectCatalogEntry.fromJson(Map<String, Object?> json) {
    final id = _stringValue(json['id']);
    final title = _stringValue(json['title']);
    final status = _stringValue(json['status']);
    return ProjectCatalogEntry(
      id: id,
      title: title.isEmpty ? id : title,
      status: status.isEmpty ? 'active' : status,
      memoryScope: _stringValue(json['memoryScope']),
      defaultMemoryPolicy: MikuMemoryPolicy.fromJson(
        json['defaultMemoryPolicy'],
      ),
      projectUri: _stringValue(json['projectUri']),
      linkedFoldersUri: _stringValue(json['linkedFoldersUri']),
      linkedFolderUris: _stringList(json['linkedFolderUris']),
      poolId: _nullableString(json['poolId']),
    );
  }

  final String id;
  final String title;
  final String status;
  final String memoryScope;
  final MikuMemoryPolicy defaultMemoryPolicy;
  final String projectUri;
  final String linkedFoldersUri;
  final List<String> linkedFolderUris;

  /// The memory pool this project currently belongs to (§30.7), if any.
  final String? poolId;

  /// True when the project has at least one attached linked folder (§30).
  bool get hasLinkedFolder => linkedFolderUris.isNotEmpty;

  /// The flat folder root shown in the picker; empty for a folderless project.
  String get rootUri =>
      linkedFolderUris.isNotEmpty ? linkedFolderUris.first : '';

  ProjectCatalogEntry copyWithPoolId(String? poolId) => ProjectCatalogEntry(
    id: id,
    title: title,
    status: status,
    memoryScope: memoryScope,
    defaultMemoryPolicy: defaultMemoryPolicy,
    projectUri: projectUri,
    linkedFoldersUri: linkedFoldersUri,
    linkedFolderUris: linkedFolderUris,
    poolId: poolId,
  );
}

/// A memory pool entity (§30.7): a symmetric group of projects whose recall fan-out includes each
/// other's active scope. A project belongs to at most one active pool at a time.
class MikuMemoryPool {
  const MikuMemoryPool({
    required this.id,
    required this.title,
    required this.status,
  });

  factory MikuMemoryPool.fromJson(Map<String, Object?> json) {
    final id = _stringValue(json['id']);
    final title = _stringValue(json['title']);
    final status = _stringValue(json['status']);
    return MikuMemoryPool(
      id: id,
      title: title.isEmpty ? id : title,
      status: status.isEmpty ? 'active' : status,
    );
  }

  final String id;
  final String title;
  final String status;
}

class MikuResourceEntry {
  const MikuResourceEntry({
    required this.uri,
    required this.name,
    required this.kind,
    this.title,
    this.sizeBytes,
    this.modifiedAt,
  });

  factory MikuResourceEntry.fromJson(Map<String, Object?> json) {
    return MikuResourceEntry(
      uri: _stringValue(json['uri']),
      name: _stringValue(json['name']),
      kind: _stringValue(json['kind']),
      title: _nullableString(json['title']),
      sizeBytes: _intValue(json['sizeBytes'] ?? json['size_bytes']),
      modifiedAt: _nullableString(json['modifiedAt'] ?? json['modified_at']),
    );
  }

  final String uri;
  final String name;
  final String kind;
  final String? title;
  final int? sizeBytes;
  final String? modifiedAt;

  bool get isDirectory =>
      kind == 'linked_folder' || kind == 'dir' || kind == 'virtual_dir';

  bool get isFile =>
      kind == 'file' || kind == 'text' || kind == 'drive_document';
}
