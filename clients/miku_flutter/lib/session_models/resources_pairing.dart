part of '../session_models.dart';

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
