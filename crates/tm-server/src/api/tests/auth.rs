use super::*;
use futures::StreamExt as _;
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use crate::{
    DeviceAuthConfig, FakePushProvider, InMemoryAuthDeviceStore, InMemoryPushStore, PushCipher,
    PushService,
};

mod approval;
mod authentication;
mod pairing;
mod push;
mod readiness;
