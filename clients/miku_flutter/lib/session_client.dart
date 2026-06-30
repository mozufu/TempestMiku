import 'session_client_stub.dart'
    if (dart.library.html) 'session_client_web.dart' as impl;
import 'session_models.dart';

MikuSessionClient createDefaultClient() => impl.createClient();
