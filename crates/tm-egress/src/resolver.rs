use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::LazyLock,
};

use async_trait::async_trait;
use ipnet::IpNet;

use crate::{EgressError, Result};

static DENIED_NETWORKS: LazyLock<Vec<IpNet>> = LazyLock::new(|| {
    [
        "0.0.0.0/8",
        "10.0.0.0/8",
        "100.64.0.0/10",
        "127.0.0.0/8",
        "169.254.0.0/16",
        "172.16.0.0/12",
        "192.0.0.0/24",
        "192.0.2.0/24",
        "192.168.0.0/16",
        "198.18.0.0/15",
        "198.51.100.0/24",
        "203.0.113.0/24",
        "224.0.0.0/4",
        "240.0.0.0/4",
        "::/128",
        "::1/128",
        "::ffff:0:0/96",
        "64:ff9b:1::/48",
        "100::/64",
        "2001:db8::/32",
        "fc00::/7",
        "fe80::/10",
        "ff00::/8",
    ]
    .into_iter()
    .map(|network| network.parse().expect("static denied network is valid"))
    .collect()
});

#[async_trait]
pub trait DnsResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDnsResolver;

#[async_trait]
impl DnsResolver for SystemDnsResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        let resolved = tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| EgressError::Dns)?;
        let addresses = resolved
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            Err(EgressError::Dns)
        } else {
            Ok(addresses)
        }
    }
}

/// Reject every non-public answer before it reaches reqwest's connector. The validated addresses
/// are subsequently pinned into the per-hop client, so a second resolver answer cannot rebind the
/// already authorized request.
pub fn validate_resolved_addresses(
    addresses: &[SocketAddr],
    expected_port: u16,
    allow_private_ips: bool,
) -> Result<Vec<SocketAddr>> {
    if addresses.is_empty() {
        return Err(EgressError::Dns);
    }
    let mut validated = BTreeSet::new();
    for address in addresses {
        if address.port() != expected_port {
            return Err(EgressError::Denied(
                "DNS answer changed the destination port".into(),
            ));
        }
        let ip = normalize_ip(address.ip());
        if hard_denied(ip) || (!allow_private_ips && denied_network(ip)) {
            return Err(EgressError::Denied(
                "DNS answer is private, loopback, link-local, metadata, or non-routable".into(),
            ));
        }
        validated.insert(SocketAddr::new(ip, expected_port));
    }
    Ok(validated.into_iter().collect())
}

fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ipv6) => ipv6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ipv6)),
        other => other,
    }
}

fn hard_denied(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_link_local()
                || ip == Ipv4Addr::new(100, 100, 100, 200)
                || ip == Ipv4Addr::new(168, 63, 129, 16)
        }
        IpAddr::V6(ip) => {
            ip.is_unspecified()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || ip == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)
        }
    }
}

fn denied_network(ip: IpAddr) -> bool {
    DENIED_NETWORKS.iter().any(|network| network.contains(&ip))
        || match ip {
            IpAddr::V4(ip) => ipv4_special(ip),
            IpAddr::V6(ip) => ipv6_special(ip),
        }
}

fn ipv4_special(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_documentation()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
}

fn ipv6_special(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        || (ip.segments()[0] & 0xffc0) == 0xfe80
}
