part of '../session_client_io.dart';

extension _NativeDriveClient on NativeMikuSessionClient {
  Future<List<ProjectCatalogEntry>> _listProjectsImpl() async {
    final json = await _request('GET', '/projects');
    return ((json['projects'] as List?) ?? const [])
        .whereType<Map>()
        .map(
          (item) => ProjectCatalogEntry.fromJson(item.cast<String, Object?>()),
        )
        .toList();
  }

  Future<String> _setSessionScopeImpl(String sessionId, String scope) async {
    final json = await _request(
      'POST',
      '/sessions/$sessionId/scope',
      body: {'scope': scope},
    );
    return (json['memoryScope'] as String?) ??
        (json['memory_scope'] as String?) ??
        scope;
  }

  Future<ProjectOverview> _projectOverviewImpl(String sessionId) async {
    final json = await _request('GET', '/sessions/$sessionId/project');
    return ProjectOverview.fromJson(json);
  }

  Future<DriveFeed> _driveFeedImpl(
    String sessionId, {
    int limit = 20,
    String? project,
  }) async {
    final trimmedProject = project?.trim();
    final query =
        Uri(
          queryParameters: {
            'limit': '$limit',
            if (trimmedProject != null && trimmedProject.isNotEmpty)
              'project': trimmedProject,
          },
        ).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/drive/feed?$query',
    );
    return DriveFeed.fromJson(json);
  }

  Future<ResourcePreview> _previewResourceImpl(
    String sessionId,
    String uri,
  ) async {
    final query = Uri(queryParameters: {'uri': uri}).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/resources/preview?$query',
    );
    return _resourcePreviewFromJson(json, uri);
  }

  Future<ResourcePreview> _resolveResourceImpl(
    String sessionId,
    String uri, {
    String? selector,
  }) async {
    final query =
        Uri(
          queryParameters: {
            'uri': uri,
            if (selector != null) 'selector': selector,
          },
        ).query;
    final json = await _request(
      'GET',
      '/sessions/$sessionId/resources/resolve?$query',
    );
    return _resourcePreviewFromJson(json, uri);
  }

  Future<List<MikuResourceEntry>> _listResourcesImpl(
    String sessionId,
    String uri,
  ) async {
    final query = Uri(queryParameters: {'uri': uri}).query;
    final json = await _requestList(
      'GET',
      '/sessions/$sessionId/resources/list?$query',
    );
    return json
        .whereType<Map>()
        .map((item) => MikuResourceEntry.fromJson(item.cast<String, Object?>()))
        .toList();
  }

  Future<int> _assignSessionToProjectImpl(
    String projectId,
    String sessionId,
  ) async {
    final json = await _request(
      'POST',
      '/projects/$projectId/sessions/$sessionId',
      body: const {},
    );
    return (json['assigned'] as num?)?.toInt() ?? 0;
  }

  ResourcePreview _resourcePreviewFromJson(
    Map<String, Object?> json,
    String uri,
  ) {
    return ResourcePreview(
      uri: json['uri'] as String? ?? uri,
      kind: json['kind'] as String? ?? '',
      mime: json['mime'] as String? ?? '',
      title: json['title'] as String?,
      sizeBytes: json['size_bytes'] as int? ?? json['sizeBytes'] as int? ?? 0,
      preview: json['preview'] as String? ?? '',
      content: json['content'] as String? ?? '',
      selector: _nullableString(json['selector']),
      hasMore: json['has_more'] as bool? ?? json['hasMore'] as bool? ?? false,
    );
  }
}
