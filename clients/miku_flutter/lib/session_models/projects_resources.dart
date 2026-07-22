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

  /// True when the project has at least one attached linked folder (§30).
  bool get hasLinkedFolder => linkedFolderUris.isNotEmpty;

  /// The flat folder root shown in the picker; empty for a folderless project.
  String get rootUri =>
      linkedFolderUris.isNotEmpty ? linkedFolderUris.first : '';
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
