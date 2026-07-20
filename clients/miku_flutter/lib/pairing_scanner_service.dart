import 'pairing_scanner_service_platform.dart';
import 'pairing_scanner_service_stub.dart'
    if (dart.library.io) 'pairing_scanner_service_io.dart'
    as platform;

export 'pairing_scanner_service_platform.dart';

PairingScannerService createPairingScannerService() =>
    platform.createPlatformPairingScannerService();
