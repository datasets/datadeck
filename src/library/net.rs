// The bookmark itself may be on an intranet. All other origins selected by its
// markup or redirects must resolve to a public address. Validate at connect time.
use super::http_url;
use std::net::IpAddr;
use std::time::Duration;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::{Origin, Url};

fn public_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(a == 0
                || a >= 224
                || v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_documentation()
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && matches!(b, 18 | 19))
                || (a == 192 && ((b == 0 && c == 0) || (b == 88 && c == 99))))
        }
        IpAddr::V6(v) => {
            let [a, b, ..] = v.segments();
            // Global unicast, excluding protocol assignments, 6to4 and documentation.
            (0x2000..0x4000).contains(&a)
                && a != 0x2002
                && !(a == 0x2001 && (b < 0x200 || b == 0xdb8))
                && !(a == 0x3fff && b < 0x1000)
        }
    }
}

#[derive(Debug)]
struct BookmarkResolver(Origin);

impl Resolver for BookmarkResolver {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let url = http_url(&uri.to_string()).map_err(|_| ureq::Error::HostNotFound)?;
        let resolved = DefaultResolver::default().resolve(uri, config, timeout)?;
        if self.0 == url.origin() {
            return Ok(resolved);
        }
        let mut allowed = self.empty();
        for &addr in resolved.iter() {
            if public_ip(addr.ip()) {
                allowed.push(addr);
            }
        }
        if allowed.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(allowed)
    }
}

pub(super) fn agent(url: &Url) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(4)
        .timeout_global(Some(Duration::from_secs(12)))
        .build();
    ureq::Agent::with_parts(
        config,
        DefaultConnector::default(),
        BookmarkResolver(url.origin()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cross_origin_addresses_must_be_public() {
        for addr in [
            "127.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "::",
            "169.254.169.254",
            "10.0.0.1",
            "192.168.7.1",
            "100.64.0.2",
            "198.19.2.2",
            "2001:db8::1",
            "2002:7f00:1::1",
            "ff02::1",
            "fe80::1",
            "fd01::1",
        ] {
            assert!(!public_ip(addr.parse().unwrap()), "{addr}");
        }
        for addr in ["1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_ip(addr.parse().unwrap()), "{addr}");
        }
    }
}
