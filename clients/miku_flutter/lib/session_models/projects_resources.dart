part of '../session_models.dart';

class ProjectCatalogEntry {
  const ProjectCatalogEntry({
    required this.id,
    required this.memoryScope,
    required this.projectUri,
    required this.linkedFoldersUri,
  });

  factory ProjectCatalogEntry.fromJson(Map<String, Object?> json) {
    return ProjectCatalogEntry(
      id: _stringValue(json['id']),
      memoryScope: _stringValue(json['memoryScope']),
      projectUri: _stringValue(json['projectUri']),
      linkedFoldersUri: _stringValue(json['linkedFoldersUri']),
    );
  }

  final String id;
  final String memoryScope;
  final String projectUri;
  final String linkedFoldersUri;

  String get rootUri => '$linkedFoldersUri/$id/';
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
