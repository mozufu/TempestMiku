import 'dart:io';

import 'package:flutter/services.dart';

import 'share_import_service_platform.dart';

const _shareImports = EventChannel('org.mozufu.tempestmiku/share-imports');

MikuShareImportService createShareImportService() =>
    const AndroidShareImportService();

class AndroidShareImportService implements MikuShareImportService {
  const AndroidShareImportService();

  @override
  bool get isSupported => Platform.isAndroid;

  @override
  Stream<SharedContent> get imports {
    if (!isSupported) return const Stream.empty();
    return _shareImports.receiveBroadcastStream().map((event) {
      if (event is! Map) {
        throw const FormatException('invalid Android share event');
      }
      return SharedContent.fromEvent(event.cast<Object?, Object?>());
    });
  }
}
