//! macOS' Local Network permission.
//!
//! macOS 15 and later ask whether an app (and the programs it runs, such as `ssh`) may reach
//! devices on the local network. Until the user answers, connections there fail at once, and
//! allowing it does not bring those back. So a connection that failed this way is tried again
//! for a while, and what it reports in the end says where to look.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs as _};
use std::time::Duration;

use crate::RemoteError;

/// The pauses before trying again: 15 s in all to read and answer macOS' question.
const PAUSES: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(3),
    Duration::from_secs(4),
    Duration::from_secs(5),
];

/// Runs `connect`, on macOS again (after a `pause`) while it fails the way connections do that
/// the Local Network permission blocks.
pub(crate) fn retrying(
    connect: impl FnMut() -> Result<(), RemoteError>,
    pause: impl FnMut(Duration),
) -> Result<(), RemoteError> {
    retry_blocked(
        connect,
        |error| cfg!(target_os = "macos") && may_be_blocked(error, &lookup),
        pause,
    )
}

fn retry_blocked(
    mut connect: impl FnMut() -> Result<(), RemoteError>,
    blocked: impl Fn(&RemoteError) -> bool,
    mut pause: impl FnMut(Duration),
) -> Result<(), RemoteError> {
    let mut pauses = PAUSES.into_iter();
    loop {
        match connect() {
            Err(error) if blocked(&error) => match (pauses.next(), error) {
                (Some(time), error) => {
                    log::debug!("connecting again in {time:?}: {error}");
                    pause(time);
                }
                (None, RemoteError::Unreachable(detail)) => {
                    return Err(RemoteError::LocalNetwork(detail));
                }
                (None, error) => return Err(error),
            },
            result => return result,
        }
    }
}

/// Whether macOS may have blocked the connection that failed with `error`: it found no way to
/// a host on the local network (which is also what a host that is off looks like).
fn may_be_blocked(error: &RemoteError, lookup: &impl Fn(&str) -> Vec<IpAddr>) -> bool {
    let RemoteError::Unreachable(detail) = error else {
        return false;
    };
    if let Some(rest) = detail.strip_prefix("Could not resolve hostname ") {
        // Bonjour names are looked up on the local network, so that is blocked too.
        return rest
            .split_once(": ")
            .is_some_and(|(host, _)| is_bonjour(host));
    }
    if !["No route to host", "Host is down"]
        .iter()
        .any(|reason| detail.contains(reason))
    {
        return false;
    }
    // `connect to host <host> port <port>: <reason>`, also for a jump host.
    detail
        .strip_prefix("connect to host ")
        .and_then(|rest| rest.split_once(" port "))
        .is_some_and(|(host, _)| is_local(host, lookup))
}

fn is_local(host: &str, lookup: &impl Fn(&str) -> Vec<IpAddr>) -> bool {
    let address = host.split_once('%').map_or(host, |(address, _)| address);
    match address.parse() {
        Ok(ip) => is_local_ip(ip),
        Err(_) => is_bonjour(host) || lookup(host).into_iter().any(is_local_ip),
    }
}

fn is_bonjour(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    host.len()
        .checked_sub(".local".len())
        .and_then(|start| host.get(start..))
        .is_some_and(|end| end.eq_ignore_ascii_case(".local"))
}

fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_local_v4(ip),
        IpAddr::V6(ip) => is_local_v6(ip),
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

fn is_local_v6(ip: Ipv6Addr) -> bool {
    match ip.to_ipv4_mapped() {
        Some(ip) => is_local_v4(ip),
        None => ip.is_unicast_link_local() || ip.is_unique_local(),
    }
}

/// The addresses of `host`, as the system knows them.
fn lookup(host: &str) -> Vec<IpAddr> {
    (host, 22)
        .to_socket_addrs()
        .map_or_default(|addresses| addresses.map(|address| address.ip()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn names(host: &str) -> Vec<IpAddr> {
        match host {
            "home-ubuntu-server" => vec!["192.168.1.20".parse().unwrap()],
            "dgx.example.com" => vec!["93.184.216.34".parse().unwrap()],
            _ => Vec::new(),
        }
    }

    fn blocked(stderr: &str) -> bool {
        may_be_blocked(&crate::classify(stderr), &names)
    }

    #[test]
    fn local_hosts_without_a_route_may_be_blocked() {
        for host in [
            "192.168.1.20",
            "10.0.0.9",
            "172.16.4.1",
            "169.254.10.1",
            "fe80::1%en0",
            "fd12:3456::1",
            "::ffff:192.168.1.20",
            "host.local",
            "Host.Local.",
            "home-ubuntu-server",
        ] {
            for reason in ["No route to host", "Host is down"] {
                let stderr = format!("ssh: connect to host {host} port 22: {reason}\r\n");
                assert!(blocked(&stderr), "{stderr}");
            }
        }
        // Through a jump host, ssh adds its own message.
        assert!(blocked(
            "ssh: connect to host 192.168.1.20 port 2222: No route to host\n\
             kex_exchange_identification: Connection closed by remote host\n\
             Connection closed by UNKNOWN port 65535\n"
        ));
        assert!(blocked(
            "ssh: Could not resolve hostname host.local: nodename nor servname provided, or not \
             known\n"
        ));
    }

    #[test]
    fn other_failures_are_not_blocked() {
        for stderr in [
            "ssh: connect to host 93.184.216.34 port 22: No route to host\n",
            "ssh: connect to host dgx.example.com port 22: No route to host\n",
            "ssh: connect to host gone.example.com port 22: No route to host\n",
            "ssh: connect to host 127.0.0.1 port 22: Host is down\n",
            // A VPN's address: the permission is not about those.
            "ssh: connect to host 100.64.0.7 port 22: No route to host\n",
            "ssh: connect to host 192.168.1.20 port 22: Connection refused\n",
            "ssh: connect to host 192.168.1.20 port 22: Operation timed out\n",
            "ssh: Could not resolve hostname home: nodename nor servname provided, or not known\n",
            "ssh: Could not resolve hostname local: nodename nor servname provided, or not known\n",
            "me@192.168.1.20: Permission denied (publickey,password).\n",
            "Host key verification failed.\n",
        ] {
            assert!(!blocked(stderr), "{stderr}");
        }
    }

    fn no_route() -> RemoteError {
        RemoteError::Unreachable("connect to host 10.0.0.9 port 22: No route to host".to_owned())
    }

    /// Runs [`retry_blocked`] with one result per attempt; also returns the pauses made.
    fn run(results: Vec<Result<(), RemoteError>>) -> (Result<(), RemoteError>, Vec<Duration>) {
        let mut results = results.into_iter();
        let mut pauses = Vec::new();
        let result = retry_blocked(
            || results.next().expect("tried too often"),
            |error| may_be_blocked(error, &names),
            |time| pauses.push(time),
        );
        assert!(results.next().is_none(), "gave up too early");
        (result, pauses)
    }

    #[test]
    fn a_blocked_connection_is_tried_again() {
        let (result, pauses) = run(vec![Err(no_route()), Err(no_route()), Ok(())]);
        assert!(result.is_ok());
        assert_eq!(pauses, PAUSES[..2]);

        // Once allowed, what comes next counts.
        let (result, pauses) = run(vec![Err(no_route()), Err(RemoteError::HostKeyUnknown)]);
        assert!(matches!(result, Err(RemoteError::HostKeyUnknown)));
        assert_eq!(pauses, PAUSES[..1]);
    }

    #[test]
    fn a_blocked_connection_is_given_up_with_a_hint() {
        let (result, pauses) = run((0..=PAUSES.len()).map(|_| Err(no_route())).collect());
        assert_eq!(pauses, PAUSES);
        let message = result.unwrap_err().to_string();
        assert!(message.contains("10.0.0.9 port 22: No route to host"));
        assert!(message.contains("Privacy & Security → Local Network"));
    }

    #[test]
    fn other_results_come_back_at_once() {
        for first in [
            Ok(()),
            Err(RemoteError::HostKeyChanged),
            Err(RemoteError::Unreachable(
                "connect to host 10.0.0.9 port 22: Connection refused".to_owned(),
            )),
        ] {
            let expected = first.as_ref().map_err(ToString::to_string).err();
            let (result, pauses) = run(vec![first]);
            assert_eq!(result.err().map(|error| error.to_string()), expected);
            assert_eq!(pauses, []);
        }
    }
}
