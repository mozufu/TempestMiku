import 'share_import_service_stub.dart'
    if (dart.library.io) 'share_import_service_io.dart'
    as impl;
import 'share_import_service_platform.dart';

export 'share_import_service_platform.dart';

MikuShareImportService createShareImportService() =>
    impl.createShareImportService();
