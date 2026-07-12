import 'package:flutter/widgets.dart';

import 'notification_service_platform.dart';
import 'notification_service_stub.dart'
    if (dart.library.io) 'notification_service_io.dart'
    as impl;

export 'notification_service_platform.dart';

/// Keeps system-notification details outside the shared Web/PWA client path.
///
/// Approval details remain in the authenticated application view. A system
/// notification only tells the owner to reopen TempestMiku; it never contains
/// an action, capability scope, session transcript, or credential.
MikuNotificationService createNotificationService() =>
    impl.createNotificationService();

/// Android should show an alert only when the authenticated view is not
/// visible. The visible approval card remains the source of truth.
bool shouldNotifyApproval(AppLifecycleState state) =>
    state != AppLifecycleState.resumed;
