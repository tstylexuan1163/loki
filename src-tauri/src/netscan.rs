//! Finds SSH and telnet services on a block of addresses, so a switch
//! management subnet can be added in one go instead of host by host.
//!
//! Plain TCP connects rather than ping: ICMP needs raw sockets (elevated
//! rights on macOS and Windows) and many switches drop it anyway, while the
//! management ports are exactly what we want to know about.

use futures::StreamExt;
use serde::Serialize;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Larger than any management subnet worth scanning in one go (a /20), small
/// enough that a typo like /8 is refused instead of running for an hour.
pub const MAX_TARGETS: usize = 4096;

/// Addresses probed at once. Each holds up to two sockets, and an app started
/// from Finder gets a soft limit of 256 open files on macOS, so this stays
/// well under half of that.
const CONCURRENCY: usize = 64;

/// How long an open port gets to say who it is.
const SSH_BANNER_WAIT: Duration = Duration::from_millis(1500);
const TELNET_BANNER_WAIT: Duration = Duration::from_millis(900);

/// Parses what the user typed into a list of addresses.
///
/// Accepts any mix of `10.1.100.0/24`, `10.1.100.10-50`,
/// `10.1.100.10-10.1.101.20` and single addresses, separated by commas,
/// semicolons or whitespace. Network and broadcast addresses of a CIDR block
/// are left out, since no device answers on them.
pub fn parse_targets(spec: &str) -> Result<Vec<Ipv4Addr>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let tokens = spec
        .split(|c: char| c == ',' || c == '，' || c == ';' || c.is_whitespace())
        .filter(|t| !t.is_empty());
    for token in tokens {
        let (start, end) = parse_token(token)?;
        if (end - start) as usize >= MAX_TARGETS {
            return Err(too_many());
        }
        for n in start..=end {
            if seen.insert(n) {
                out.push(Ipv4Addr::from(n));
                if out.len() > MAX_TARGETS {
                    return Err(too_many());
                }
            }
        }
    }
    if out.is_empty() {
        return Err("请填写要扫描的网段，例如 192.168.1.0/24".into());
    }
    Ok(out)
}

fn too_many() -> String {
    format!("地址太多（超过 {MAX_TARGETS} 个），请缩小范围，例如一次扫描一个 /24")
}

/// One token as an inclusive range of addresses in host order.
fn parse_token(token: &str) -> Result<(u32, u32), String> {
    let bad = || format!("无法识别「{token}」，示例：192.168.1.0/24、10.1.100.1-254");
    let ip = |s: &str| {
        s.trim()
            .parse::<Ipv4Addr>()
            .map(u32::from)
            .map_err(|_| bad())
    };

    if let Some((addr, prefix)) = token.split_once('/') {
        let prefix: u32 = prefix.parse().map_err(|_| bad())?;
        if prefix > 32 {
            return Err(bad());
        }
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        let network = ip(addr)? & mask;
        let broadcast = network | !mask;
        // /31 and /32 have no network or broadcast address to skip.
        return Ok(if prefix >= 31 {
            (network, broadcast)
        } else {
            (network + 1, broadcast - 1)
        });
    }

    if let Some((from, to)) = token.split_once('-') {
        let start = ip(from)?;
        let end = if to.contains('.') {
            ip(to)?
        } else {
            // A bare number replaces the last octet: 10.1.100.10-50.
            let last: u8 = to.trim().parse().map_err(|_| bad())?;
            (start & 0xffff_ff00) | u32::from(last)
        };
        if end < start {
            return Err(format!("「{token}」的结束地址比起始地址小"));
        }
        return Ok((start, end));
    }

    let one = ip(token)?;
    Ok((one, one))
}

/// What the scan found on one address. Only addresses with at least one open
/// port are reported.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScanHit {
    pub address: String,
    pub ssh: bool,
    pub telnet: bool,
    /// The most telling line the device sent on connect: the SSH version
    /// string, or the first readable line of the telnet greeting.
    pub banner: Option<String>,
    /// Guessed from the banner; `None` when it gives nothing away.
    pub vendor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ScanEvent {
    Progress {
        scan_id: String,
        done: usize,
        total: usize,
    },
    Found {
        scan_id: String,
        hit: ScanHit,
    },
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub ssh_port: Option<u16>,
    pub telnet_port: Option<u16>,
    /// Per connection attempt. A closed port answers at once; this only
    /// bounds addresses where nothing answers at all.
    pub timeout: Duration,
}

/// Probes every target and returns the hits sorted by address. Stops early
/// when `cancel` is set, returning what was found so far.
pub async fn scan(
    scan_id: &str,
    targets: Vec<Ipv4Addr>,
    opts: ScanOptions,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(ScanEvent),
) -> Vec<ScanHit> {
    let total = targets.len();
    let mut done = 0;
    let mut hits = Vec::new();
    let mut last_report = Instant::now() - Duration::from_secs(1);

    let mut probes = futures::stream::iter(targets)
        .map(|addr| {
            let opts = opts.clone();
            let cancel = cancel.clone();
            async move {
                if cancel.load(Ordering::SeqCst) {
                    return None;
                }
                probe_host(addr, &opts).await
            }
        })
        .buffer_unordered(CONCURRENCY);

    while let Some(result) = probes.next().await {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        done += 1;
        if let Some(hit) = result {
            on_event(ScanEvent::Found {
                scan_id: scan_id.to_string(),
                hit: hit.clone(),
            });
            hits.push(hit);
        }
        // A /20 finishes thousands of probes; the UI only needs a few
        // updates a second, plus the final count.
        if done == total || last_report.elapsed() >= Duration::from_millis(100) {
            last_report = Instant::now();
            on_event(ScanEvent::Progress {
                scan_id: scan_id.to_string(),
                done,
                total,
            });
        }
    }

    hits.sort_by_key(|h| h.address.parse::<Ipv4Addr>().map(u32::from).unwrap_or(0));
    hits
}

async fn probe_host(addr: Ipv4Addr, opts: &ScanOptions) -> Option<ScanHit> {
    let ssh = async {
        match opts.ssh_port {
            Some(port) => probe_ssh(SocketAddr::from((addr, port)), opts.timeout).await,
            None => None,
        }
    };
    let telnet = async {
        match opts.telnet_port {
            Some(port) => probe_telnet(SocketAddr::from((addr, port)), opts.timeout).await,
            None => None,
        }
    };
    let (ssh, telnet) = tokio::join!(ssh, telnet);
    if ssh.is_none() && telnet.is_none() {
        return None;
    }

    let ssh_open = ssh.is_some();
    let telnet_open = telnet.is_some();
    let ssh_banner = ssh.flatten();
    let telnet_text = telnet.flatten();
    // Identify from everything the device said; show the SSH version string
    // when there is one, since it is the most precise.
    let vendor = ssh_banner
        .as_deref()
        .and_then(vendor_from_banner)
        .or_else(|| telnet_text.as_deref().and_then(vendor_from_banner));
    let banner = ssh_banner.or_else(|| telnet_text.as_deref().and_then(display_line));
    Some(ScanHit {
        address: addr.to_string(),
        ssh: ssh_open,
        telnet: telnet_open,
        banner,
        vendor: vendor.map(String::from),
    })
}

async fn connect(sa: SocketAddr, timeout: Duration) -> Option<TcpStream> {
    match tokio::time::timeout(timeout, TcpStream::connect(sa)).await {
        Ok(Ok(stream)) => Some(stream),
        _ => None,
    }
}

/// `None` when the port is closed; `Some(None)` when it is open but sent no
/// version string in time.
async fn probe_ssh(sa: SocketAddr, timeout: Duration) -> Option<Option<String>> {
    let mut stream = connect(sa, timeout).await?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];
    let deadline = Instant::now() + SSH_BANNER_WAIT;
    while buf.len() < 512 && !buf.contains(&b'\n') {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, stream.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => buf.extend_from_slice(&chunk[..n]),
            _ => break,
        }
    }
    // Servers may send other lines before the version string (RFC 4253 4.2).
    let text = String::from_utf8_lossy(&buf);
    let banner = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("SSH-"))
        .map(|l| truncate(l, 80));
    Some(banner)
}

/// Like `probe_ssh`, but returns the readable text of the greeting. Option
/// requests are answered (refused, mostly) so servers that wait for the
/// negotiation to settle go on to print their banner.
async fn probe_telnet(sa: SocketAddr, timeout: Duration) -> Option<Option<String>> {
    let mut stream = connect(sa, timeout).await?;
    let mut neg = crate::telnet::Negotiation::default();
    let mut text = Vec::new();
    let mut chunk = [0u8; 512];
    let deadline = Instant::now() + TELNET_BANNER_WAIT;
    while text.len() < 2048 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, stream.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => {
                let (data, replies) = crate::telnet::strip_commands(&chunk[..n], &mut neg);
                text.extend_from_slice(&data);
                if !replies.is_empty() && stream.write_all(&replies).await.is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&text).trim().to_string();
    Some((!text.is_empty()).then_some(text))
}

/// The first line of a greeting worth showing: not a row of asterisks, not a
/// bare `Username:` prompt.
fn display_line(text: &str) -> Option<String> {
    text.lines()
        .map(|l| l.trim_matches(|c: char| c == '*' || c == '-' || c == '=' || c.is_whitespace()))
        .filter(|l| l.chars().filter(|c| c.is_alphanumeric()).count() >= 3)
        .find(|l| !l.ends_with(':') || l.contains(' '))
        .map(|l| truncate(l, 80))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Names the vendor from what a device says on connect. Order matters where
/// one name is part of another's banner.
pub fn vendor_from_banner(banner: &str) -> Option<&'static str> {
    let b = banner.to_lowercase();
    const TABLE: &[(&[&str], &str)] = &[
        (&["comware", "h3c"], "H3C"),
        (&["huawei", "vrp"], "华为"),
        (&["ruijie", "rgos"], "锐捷"),
        (&["zxr10", "zte"], "中兴"),
        (&["cisco", "user access verification"], "思科"),
        (&["junos", "juniper"], "Juniper"),
        (&["mikrotik", "rosssh"], "MikroTik"),
        (&["fortinet", "fortigate"], "Fortinet"),
        (&["maipu"], "迈普"),
        (&["dptech"], "迪普"),
        (&["openssh"], "OpenSSH"),
        (&["dropbear"], "Dropbear"),
    ];
    TABLE
        .iter()
        .find(|(needles, _)| needles.iter().any(|n| b.contains(n)))
        .map(|(_, name)| *name)
}

/// Builds a host name from the user's pattern. `{ip}`, `{last}` (the last
/// octet) and `{vendor}` are replaced; separators left dangling by an unknown
/// vendor are trimmed, and an empty result falls back to the address.
pub fn render_name(template: &str, address: &str, vendor: Option<&str>) -> String {
    let last = address.rsplit('.').next().unwrap_or(address);
    let name = template
        .replace("{ip}", address)
        .replace("{last}", last)
        .replace("{vendor}", vendor.unwrap_or(""));
    let name = name
        .trim_matches(|c: char| c == '-' || c == '_' || c.is_whitespace())
        .replace("--", "-")
        .replace("__", "_");
    if name.is_empty() {
        address.to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(spec: &str) -> Vec<String> {
        parse_targets(spec)
            .unwrap()
            .iter()
            .map(|a| a.to_string())
            .collect()
    }

    #[test]
    fn cidr_skips_network_and_broadcast() {
        let list = ips("10.1.100.0/24");
        assert_eq!(list.len(), 254);
        assert_eq!(list[0], "10.1.100.1");
        assert_eq!(list[253], "10.1.100.254");
        // Any address inside the block names the same block.
        assert_eq!(ips("10.1.100.77/24"), list);
        assert_eq!(ips("192.168.1.4/30"), ["192.168.1.5", "192.168.1.6"]);
        assert_eq!(ips("192.168.1.9/32"), ["192.168.1.9"]);
        assert_eq!(ips("192.168.1.8/31"), ["192.168.1.8", "192.168.1.9"]);
    }

    #[test]
    fn ranges_and_lists() {
        assert_eq!(ips("10.0.0.10-12"), ["10.0.0.10", "10.0.0.11", "10.0.0.12"]);
        assert_eq!(
            ips("10.0.0.254-10.0.1.1"),
            ["10.0.0.254", "10.0.0.255", "10.0.1.0", "10.0.1.1"]
        );
        // Mixed separators, duplicates once, order kept.
        assert_eq!(
            ips("10.0.0.5，10.0.0.1-2; 10.0.0.2\n10.0.0.5"),
            ["10.0.0.5", "10.0.0.1", "10.0.0.2"]
        );
    }

    #[test]
    fn refuses_nonsense_and_huge_ranges() {
        for bad in [
            "",
            "  ",
            "10.0.0",
            "10.0.0.1/33",
            "abc",
            "10.0.0.9-3",
            "10.0.0.1-300",
        ] {
            assert!(parse_targets(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(parse_targets("10.0.0.0/16").is_err());
        assert!(parse_targets("10.0.0.0/20").is_ok());
        // Many small pieces add up too.
        let pieces: Vec<String> = (0..17).map(|i| format!("10.{i}.0.0/24")).collect();
        assert!(parse_targets(&pieces.join(",")).is_err());
    }

    #[test]
    fn vendors_from_real_banners() {
        assert_eq!(vendor_from_banner("SSH-2.0-Comware-7.1.064"), Some("H3C"));
        assert_eq!(vendor_from_banner("SSH-2.0-HUAWEI-1.5"), Some("华为"));
        assert_eq!(vendor_from_banner("SSH-2.0-RGOS_SSH"), Some("锐捷"));
        assert_eq!(vendor_from_banner("SSH-1.99-Cisco-1.25"), Some("思科"));
        assert_eq!(vendor_from_banner("User Access Verification"), Some("思科"));
        assert_eq!(
            vendor_from_banner("* Copyright (c) 2004-2021 New H3C Technologies Co., Ltd."),
            Some("H3C")
        );
        assert_eq!(
            vendor_from_banner("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3"),
            Some("OpenSSH")
        );
        assert_eq!(vendor_from_banner("SSH-2.0--"), None);
    }

    #[test]
    fn greeting_line_skips_decoration_and_prompts() {
        let h3c = "******************************\r\n* Copyright (c) 2004-2021 New H3C Technologies *\r\n\r\nLogin: ";
        assert_eq!(
            display_line(h3c).as_deref(),
            Some("Copyright (c) 2004-2021 New H3C Technologies")
        );
        assert_eq!(display_line("\r\nUsername:").as_deref(), None);
        assert_eq!(
            display_line("Login authentication\r\n\r\nUsername:").as_deref(),
            Some("Login authentication")
        );
    }

    #[test]
    fn names_from_template() {
        assert_eq!(
            render_name("SW-{ip}", "10.1.100.23", None),
            "SW-10.1.100.23"
        );
        assert_eq!(
            render_name("{vendor}-{last}", "10.1.100.23", Some("H3C")),
            "H3C-23"
        );
        assert_eq!(render_name("{vendor}-{last}", "10.1.100.23", None), "23");
        assert_eq!(
            render_name("core-{vendor}-{last}", "10.1.100.23", None),
            "core-23"
        );
        assert_eq!(render_name("  ", "10.1.100.23", None), "10.1.100.23");
    }

    #[tokio::test]
    async fn finds_an_ssh_banner_and_a_closed_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let _ = sock.write_all(b"SSH-2.0-Comware-7.1.064\r\n").await;
            }
        });
        // Bound then dropped: nothing listens there any more.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };

        let mut events = Vec::new();
        let hits = scan(
            "t",
            vec![Ipv4Addr::LOCALHOST],
            ScanOptions {
                ssh_port: Some(port),
                telnet_port: Some(closed),
                timeout: Duration::from_millis(500),
            },
            Arc::new(AtomicBool::new(false)),
            |e| events.push(e),
        )
        .await;

        assert_eq!(
            hits,
            vec![ScanHit {
                address: "127.0.0.1".into(),
                ssh: true,
                telnet: false,
                banner: Some("SSH-2.0-Comware-7.1.064".into()),
                vendor: Some("H3C".into()),
            }]
        );
        assert!(matches!(
            events.last(),
            Some(ScanEvent::Progress {
                done: 1,
                total: 1,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_telnet_greeting_is_read_through_negotiation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                // IAC WILL ECHO, IAC DO NAWS, then the greeting.
                let _ = sock.write_all(&[255, 251, 1, 255, 253, 31]).await;
                let _ = sock
                    .write_all(b"\r\nLogin authentication\r\n\r\nUsername:")
                    .await;
                let mut sink = [0u8; 64];
                let _ = sock.read(&mut sink).await;
            }
        });

        let hits = scan(
            "t",
            vec![Ipv4Addr::LOCALHOST],
            ScanOptions {
                ssh_port: None,
                telnet_port: Some(port),
                timeout: Duration::from_millis(500),
            },
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .await;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].telnet && !hits[0].ssh);
        assert_eq!(hits[0].banner.as_deref(), Some("Login authentication"));
    }

    #[tokio::test]
    async fn cancelling_stops_the_scan() {
        let cancel = Arc::new(AtomicBool::new(true));
        let hits = scan(
            "t",
            parse_targets("10.255.0.0/24").unwrap(),
            ScanOptions {
                ssh_port: Some(22),
                telnet_port: None,
                timeout: Duration::from_secs(5),
            },
            cancel,
            |_| {},
        )
        .await;
        assert!(hits.is_empty());
    }
}
