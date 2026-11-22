//! HTTP for search backends and page reading.
//!
//! Pages are fetched through an agent whose resolver refuses non-public addresses (loopback,
//! private, link-local, …) unless `allow_private_networks` is set. The check runs on the
//! addresses the connection actually uses, for every redirect hop, so a public name that
//! resolves to (or redirects to) a private address is refused too.

use std::io::Read as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use ureq::Agent;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::config::Fetch;

/// An agent for the configured search service (which may be on the local network).
pub fn backend_agent(timeout_secs: u64, user_agent: &str) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .user_agent(user_agent)
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// An agent for pages the model asks to read.
pub fn page_agent(config: &Fetch) -> Agent {
    let builder = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(config.timeout_secs)))
        .user_agent(config.user_agent.as_str())
        .accept("text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.5")
        .max_redirects(5)
        .http_status_as_error(false);
    if config.allow_private_networks {
        return builder.build().new_agent();
    }
    // A proxy would resolve names itself, around the address check.
    let config = builder.proxy(None).build();
    Agent::with_parts(config, DefaultConnector::default(), PublicOnly::default())
}

/// Reads at most `limit` bytes of the body; `true` when it was cut off.
pub fn read_body(body: &mut ureq::Body, limit: usize) -> Result<(Vec<u8>, bool), ureq::Error> {
    let mut bytes = Vec::new();
    body.with_config()
        .reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    Ok((bytes, truncated))
}

#[derive(Debug, Default)]
struct PublicOnly(DefaultResolver);

impl Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addresses = self.0.resolve(uri, config, timeout)?;
        if let Some(blocked) = addresses.iter().find(|address| !is_public(address.ip())) {
            return Err(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} resolves to the non-public address {}; deep search only reads public \
                     web pages (set fetch.allow_private_networks to allow this)",
                    uri.host().unwrap_or_default(),
                    blocked.ip()
                ),
            )));
        }
        Ok(addresses)
    }
}

/// Whether `ip` is a globally routable unicast address.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let segments = ip.segments();
            // NAT64 (64:ff9b::/96) embeds an IPv4 address.
            if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [a, b] = segments[6].to_be_bytes();
                let [c, d] = segments[7].to_be_bytes();
                return is_public_v4(Ipv4Addr::new(a, b, c, d));
            }
            !(ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00 // unique local
                || (segments[0] & 0xffc0) == 0xfe80 // link-local
                || (segments[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
                || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
                || segments[..6] == [0, 0, 0, 0, 0, 0] // IPv4-compatible (deprecated)
                || ip == Ipv6Addr::UNSPECIFIED)
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240 // reserved
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (18..20).contains(&b))) // benchmarking
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_addresses_pass() {
        let public = ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"];
        let private = [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.50",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.1",
            "64:ff9b::a00:1",
        ];
        for ip in public {
            assert!(is_public(ip.parse().unwrap()), "{ip} should be public");
        }
        for ip in private {
            assert!(!is_public(ip.parse().unwrap()), "{ip} should be blocked");
        }
    }
}
