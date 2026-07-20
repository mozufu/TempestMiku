part of '../session_models.dart';

class ReadinessComponent {
  const ReadinessComponent({
    required this.status,
    this.reason,
    this.dimensions,
  });

  final String status;
  final String? reason;
  final int? dimensions;

  static ReadinessComponent fromJson(Object? value) {
    if (value is String) return ReadinessComponent(status: value);
    final json = _mapValue(value);
    if (json == null || json.isEmpty) {
      return const ReadinessComponent(status: 'unknown');
    }
    if (json.containsKey('status')) {
      return ReadinessComponent(
        status: _stringValue(json['status']),
        reason: _nullableString(json['reason']),
        dimensions: _intValue(json['dimensions']),
      );
    }
    final variant = json.entries.first;
    final detail = _mapValue(variant.value);
    return ReadinessComponent(
      status: variant.key,
      reason: _nullableString(detail?['reason']),
      dimensions: _intValue(detail?['dimensions']),
    );
  }

  String get detail {
    if (reason != null) return '$status: $reason';
    if (dimensions != null) return '$status ($dimensions dimensions)';
    return status;
  }
}

class ServerMemoryReadiness {
  const ServerMemoryReadiness({
    required this.schema,
    required this.pgvector,
    required this.embeddings,
  });

  final ReadinessComponent schema;
  final ReadinessComponent pgvector;
  final ReadinessComponent embeddings;

  bool get durableWritesReady => schema.status == 'ready';

  bool get denseRetrievalReady =>
      durableWritesReady &&
      pgvector.status == 'ready' &&
      embeddings.status == 'ready';

  String get detail {
    if (!durableWritesReady) return 'memory schema ${schema.detail}';
    if (!denseRetrievalReady) {
      return 'durable writes ready; dense retrieval uses '
          'pgvector ${pgvector.detail}, embeddings ${embeddings.detail}';
    }
    return 'durable writes and dense retrieval ready';
  }

  static ServerMemoryReadiness fromJson(Map<String, Object?> json) {
    return ServerMemoryReadiness(
      schema: ReadinessComponent.fromJson(json['schema']),
      pgvector: ReadinessComponent.fromJson(json['pgvector']),
      embeddings: ReadinessComponent.fromJson(json['embeddings']),
    );
  }
}

class ServerRuntimeReadiness {
  const ServerRuntimeReadiness({
    required this.role,
    required this.postgres,
    required this.migrationsApplied,
    required this.workersEnabled,
    required this.shuttingDown,
    required this.leaseReclaims,
    required this.heartbeatFailures,
    required this.linkHydrationFailures,
    this.memoryReadiness,
  });

  final String role;
  final bool postgres;
  final bool migrationsApplied;
  final bool workersEnabled;
  final bool shuttingDown;
  final int leaseReclaims;
  final int heartbeatFailures;
  final int linkHydrationFailures;
  final ServerMemoryReadiness? memoryReadiness;

  static ServerRuntimeReadiness fromJson(Map<String, Object?> json) {
    final memory = _mapValue(
      json['memoryReadiness'] ?? json['memory_readiness'],
    );
    return ServerRuntimeReadiness(
      role: _stringValue(json['role']),
      postgres: json['postgres'] == true,
      migrationsApplied:
          (json['migrationsApplied'] ?? json['migrations_applied']) == true,
      workersEnabled:
          (json['workersEnabled'] ?? json['workers_enabled']) == true,
      shuttingDown: (json['shuttingDown'] ?? json['shutting_down']) == true,
      leaseReclaims:
          _intValue(json['leaseReclaims'] ?? json['lease_reclaims']) ?? 0,
      heartbeatFailures:
          _intValue(json['heartbeatFailures'] ?? json['heartbeat_failures']) ??
          0,
      linkHydrationFailures:
          _intValue(
            json['linkHydrationFailures'] ?? json['link_hydration_failures'],
          ) ??
          0,
      memoryReadiness:
          memory == null ? null : ServerMemoryReadiness.fromJson(memory),
    );
  }
}

class ServerReadiness {
  const ServerReadiness({
    required this.status,
    required this.runtime,
    required this.selfEvolutionTier,
  });

  final String status;
  final ServerRuntimeReadiness runtime;
  final String selfEvolutionTier;

  bool get ready => status == 'ready';

  ServerMemoryReadiness? get memory => runtime.memoryReadiness;

  String get detail {
    if (runtime.shuttingDown || status == 'draining') {
      return 'server is draining active work';
    }
    if (runtime.postgres && !runtime.migrationsApplied) {
      return 'database migrations have not finished';
    }
    if (runtime.workersEnabled && !runtime.postgres) {
      return 'worker runtime requires Postgres';
    }
    final memoryReadiness = runtime.memoryReadiness;
    if (memoryReadiness != null && !memoryReadiness.durableWritesReady) {
      return 'durable memory writes are blocked: '
          '${memoryReadiness.schema.detail}';
    }
    if (ready) {
      return memoryReadiness?.detail ?? 'runtime is ready';
    }
    return status.isEmpty
        ? 'server readiness status is unavailable'
        : 'server reported $status';
  }

  static ServerReadiness fromJson(Map<String, Object?> json) {
    final runtime = _mapValue(json['runtime']) ?? const <String, Object?>{};
    final selfEvolution =
        _mapValue(json['selfEvolution'] ?? json['self_evolution']) ??
        const <String, Object?>{};
    return ServerReadiness(
      status: _stringValue(json['status']),
      runtime: ServerRuntimeReadiness.fromJson(runtime),
      selfEvolutionTier: _stringValue(selfEvolution['tier']),
    );
  }
}

class AuthDevice {
  const AuthDevice({
    required this.id,
    required this.name,
    required this.platform,
    required this.createdAt,
    required this.lastSeenAt,
    this.revokedAt,
  });

  final String id;
  final String name;
  final String platform;
  final String createdAt;
  final String lastSeenAt;
  final String? revokedAt;

  bool get isActive => revokedAt == null || revokedAt!.isEmpty;

  static AuthDevice fromJson(Map<String, Object?> json) {
    return AuthDevice(
      id: _stringValue(json['id']),
      name: _stringValue(json['name']),
      platform: _stringValue(json['platform']),
      createdAt: _stringValue(json['createdAt']),
      lastSeenAt: _stringValue(json['lastSeenAt']),
      revokedAt: _nullableString(json['revokedAt']),
    );
  }
}

/// Origin-bound device identity extracted from a successful `/auth/pair`
/// response. It carries no bearer token or cookie material.
class PairedAuthDeviceIdentity {
  const PairedAuthDeviceIdentity({
    required this.serverBaseUrl,
    required this.deviceId,
  });

  final String serverBaseUrl;
  final String deviceId;

  static PairedAuthDeviceIdentity fromPairResponse(
    Map<String, Object?> json, {
    required String serverBaseUrl,
  }) {
    final device = _mapValue(json['device']);
    final deviceId = _stringValue(device?['id']).trim();
    if (!_isValidPairedDeviceId(deviceId)) {
      throw const FormatException(
        'pairing response did not include a valid device id',
      );
    }
    return PairedAuthDeviceIdentity(
      serverBaseUrl: normalizeMikuServerBaseUrl(serverBaseUrl),
      deviceId: deviceId,
    );
  }

  static PairedAuthDeviceIdentity? fromStored({
    required String? serverBaseUrl,
    required String? deviceId,
  }) {
    if (serverBaseUrl == null || deviceId == null) return null;
    final normalizedId = deviceId.trim();
    if (!_isValidPairedDeviceId(normalizedId)) return null;
    try {
      return PairedAuthDeviceIdentity(
        serverBaseUrl: normalizeMikuServerBaseUrl(serverBaseUrl),
        deviceId: normalizedId,
      );
    } on FormatException {
      return null;
    }
  }

  bool matchesServer(String serverBaseUrl) {
    try {
      return this.serverBaseUrl == normalizeMikuServerBaseUrl(serverBaseUrl);
    } on FormatException {
      return false;
    }
  }
}

bool _isValidPairedDeviceId(String value) =>
    value.isNotEmpty && value.length <= 128 && !value.contains(RegExp(r'\s'));

class PairingCode {
  const PairingCode({
    required this.code,
    required this.pairingLink,
    required this.expiresAt,
  });

  final String code;
  final String pairingLink;
  final String expiresAt;

  static PairingCode fromJson(Map<String, Object?> json) {
    return PairingCode(
      code: _stringValue(json['code']),
      pairingLink: _stringValue(json['pairingLink']),
      expiresAt: _stringValue(json['expiresAt']),
    );
  }
}

class ServerDiagnostics {
  const ServerDiagnostics({
    required this.baseUrl,
    required this.role,
    required this.postgres,
    required this.migrationsApplied,
    required this.workersEnabled,
    required this.shuttingDown,
    required this.turnQueueDepth,
    required this.dreamQueueDepth,
    required this.schedulerQueueDepth,
    required this.approvalEffectQueueDepth,
    required this.pushQueueDepth,
    required this.pendingApprovals,
    required this.leaseReclaims,
    required this.heartbeatFailures,
    required this.linkHydrationFailures,
  });

  final String baseUrl;
  final String role;
  final bool postgres;
  final bool migrationsApplied;
  final bool workersEnabled;
  final bool shuttingDown;
  final int turnQueueDepth;
  final int dreamQueueDepth;
  final int schedulerQueueDepth;
  final int approvalEffectQueueDepth;
  final int? pushQueueDepth;
  final int pendingApprovals;
  final int leaseReclaims;
  final int heartbeatFailures;
  final int linkHydrationFailures;

  bool get operational => !shuttingDown && (!postgres || migrationsApplied);

  static ServerDiagnostics fromJson(
    Map<String, Object?> json, {
    required String baseUrl,
  }) {
    final runtime = _mapValue(json['runtime']) ?? const {};
    final queues = _mapValue(json['queues']) ?? const {};
    int depth(String key) => _intValue(_mapValue(queues[key])?['depth']) ?? 0;
    return ServerDiagnostics(
      baseUrl: baseUrl,
      role: _stringValue(runtime['role']),
      postgres: runtime['postgres'] == true,
      migrationsApplied: runtime['migrationsApplied'] == true,
      workersEnabled: runtime['workersEnabled'] == true,
      shuttingDown: runtime['shuttingDown'] == true,
      turnQueueDepth: depth('turn'),
      dreamQueueDepth: depth('dream'),
      schedulerQueueDepth: depth('scheduler'),
      approvalEffectQueueDepth: depth('approvalEffects'),
      pushQueueDepth: _intValue(_mapValue(queues['push'])?['depth']),
      pendingApprovals: _intValue(json['pendingApprovals']) ?? 0,
      leaseReclaims: _intValue(json['leaseReclaims']) ?? 0,
      heartbeatFailures: _intValue(runtime['heartbeatFailures']) ?? 0,
      linkHydrationFailures: _intValue(runtime['linkHydrationFailures']) ?? 0,
    );
  }
}
