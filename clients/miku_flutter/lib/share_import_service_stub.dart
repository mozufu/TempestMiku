import 'share_import_service_platform.dart';

MikuShareImportService createShareImportService() =>
    const UnsupportedShareImportService();

class UnsupportedShareImportService implements MikuShareImportService {
  const UnsupportedShareImportService();

  @override
  bool get isSupported => false;

  @override
  Stream<SharedContent> get imports => const Stream.empty();
}
