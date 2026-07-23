part of '../session_client_io.dart';

extension _NativePoolsClient on NativeMikuSessionClient {
  Future<List<MikuMemoryPool>> _listMemoryPoolsImpl() async {
    final json = await _request('GET', '/pools');
    return ((json['pools'] as List?) ?? const [])
        .whereType<Map>()
        .map((item) => MikuMemoryPool.fromJson(item.cast<String, Object?>()))
        .toList();
  }

  Future<MikuMemoryPool> _createMemoryPoolImpl(String id, String? title) async {
    final json = await _request(
      'POST',
      '/pools',
      body: {'id': id, if (title != null) 'title': title},
    );
    return MikuMemoryPool.fromJson(json);
  }

  Future<MikuMemoryPool> _archiveMemoryPoolImpl(String poolId) async {
    final json = await _request('POST', '/pools/$poolId/archive');
    return MikuMemoryPool.fromJson(json);
  }

  Future<ProjectCatalogEntry> _joinMemoryPoolImpl(
    String projectId,
    String poolId,
  ) async {
    final json = await _request(
      'POST',
      '/projects/$projectId/pool',
      body: {'poolId': poolId},
    );
    return ProjectCatalogEntry.fromJson(json);
  }

  Future<ProjectCatalogEntry> _leaveMemoryPoolImpl(String projectId) async {
    final json = await _request('DELETE', '/projects/$projectId/pool');
    return ProjectCatalogEntry.fromJson(json);
  }
}
