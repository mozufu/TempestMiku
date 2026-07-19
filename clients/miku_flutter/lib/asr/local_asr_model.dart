import 'local_asr_model_platform.dart';
import 'local_asr_model_stub.dart'
    if (dart.library.io) 'local_asr_model_io.dart'
    as impl;

export 'local_asr_model_platform.dart';

LocalAsrModelManager createLocalAsrModelManager() =>
    impl.createLocalAsrModelManager();
