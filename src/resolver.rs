use hickory_proto::rr::RecordType;
use hickory_resolver::config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts};
use hickory_resolver::error::ResolveError;
use hickory_resolver::lookup::Lookup;
use hickory_resolver::AsyncResolver;

use crate::config::Upstream;
use crate::resolver_runtime_provider::{ProxyConnectionProvider, ProxyRuntimeProvider};

#[derive(Clone, Debug)]
pub struct RecursiveResolver {
    pub resolver: AsyncResolver<ProxyConnectionProvider>,
}

impl RecursiveResolver {
    pub fn new(
        resolver_config: ResolverConfig,
        options: ResolverOpts,
        provider: ProxyConnectionProvider,
    ) -> Self {
        let resolver = AsyncResolver::new(resolver_config, options, provider);
        RecursiveResolver { resolver }
    }

    pub async fn resolve(
        &self,
        domain: &str,
        record_type: RecordType,
    ) -> Result<Lookup, ResolveError> {
        match record_type {
            RecordType::A | RecordType::AAAA => match self.resolver.lookup_ip(domain).await {
                Ok(res) => Ok(res.as_lookup().to_owned()),
                Err(e) => Err(e),
            },
            _ => self.resolver.lookup(domain, record_type).await,
        }
    }
}

impl From<(&Upstream, &ResolverOpts)> for RecursiveResolver {
    fn from((upstream, options): (&Upstream, &ResolverOpts)) -> Self {
        let (protocol, address, tls_host, proxy) = match upstream {
            Upstream::UdpUpstream { address, proxy } => (Protocol::Udp, address, None, proxy),
            Upstream::TcpUpstream { address, proxy } => (Protocol::Tcp, address, None, proxy),
            #[cfg(feature = "dns-over-tls")]
            Upstream::TlsUpstream {
                address,
                tls_host,
                proxy,
            } => (Protocol::Tls, address, Some(tls_host.to_owned()), proxy),
            #[cfg(feature = "dns-over-https")]
            Upstream::HttpsUpstream {
                address,
                tls_host,
                proxy,
            } => (Protocol::Https, address, Some(tls_host.to_owned()), proxy),
            #[cfg(feature = "dns-over-h3")]
            Upstream::H3Upstream {
                address,
                tls_host,
                proxy,
            } => (Protocol::H3, address, Some(tls_host.to_owned()), proxy),
        };
        let mut resolver_config = ResolverConfig::new();
        address.iter().for_each(|addr| {
            let mut name_server_config = NameServerConfig::new(*addr, protocol);
            name_server_config.tls_dns_name = tls_host.to_owned();
            resolver_config.add_name_server(name_server_config);
        });
        let runtime_provider =
            ProxyRuntimeProvider::new(proxy.to_owned().map(|p| p.parse().unwrap()));
        let provider = ProxyConnectionProvider::new(runtime_provider);
        RecursiveResolver::new(resolver_config, options.to_owned(), provider)
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use tokio::runtime::Runtime;

    use super::*;

    #[test]
    fn udp_resolver_test() {
        let dns_addr = "8.8.8.8:53".parse::<SocketAddr>().unwrap();
        let io_loop = Runtime::new().unwrap();
        let resolver: RecursiveResolver = (
            &Upstream::UdpUpstream {
                address: vec![dns_addr],
                proxy: None,
            },
            &ResolverOpts::default(),
        )
            .into();
        let lookup_future = resolver.resolve("dns.google", RecordType::A);
        let response = io_loop.block_on(lookup_future).unwrap();
        assert!(response
            .record_iter()
            .any(|r| r.data().unwrap().to_string().eq("8.8.8.8")));
    }

    #[test]
    fn tcp_resolver_test() {
        let dns_addr = "8.8.8.8:53".parse::<SocketAddr>().unwrap();
        let io_loop = Runtime::new().unwrap();
        let resolver: RecursiveResolver = (
            &Upstream::TcpUpstream {
                address: vec![dns_addr],
                proxy: None,
            },
            &ResolverOpts::default(),
        )
            .into();
        let lookup_future = resolver.resolve("dns.google", RecordType::A);
        let response = io_loop.block_on(lookup_future).unwrap();
        assert!(response
            .record_iter()
            .any(|r| r.data().unwrap().to_string().eq("8.8.8.8")));
    }

    #[cfg(feature = "dns-over-tls")]
    #[test]
    fn tls_resolver_test() {
        let dns_addr = "8.8.8.8:853".parse::<SocketAddr>().unwrap();
        let dns_host = String::from("dns.google");
        let io_loop = Runtime::new().unwrap();
        let resolver: RecursiveResolver = (
            &Upstream::TlsUpstream {
                address: vec![dns_addr],
                proxy: None,
                tls_host: dns_host,
            },
            &ResolverOpts::default(),
        )
            .into();
        let lookup_future = resolver.resolve("dns.google", RecordType::A);
        let response = io_loop.block_on(lookup_future).unwrap();
        assert!(response
            .record_iter()
            .any(|r| r.data().unwrap().to_string().eq("8.8.8.8")));
    }

    #[cfg(feature = "dns-over-https")]
    #[test]
    fn https_resolver_test() {
        let dns_addr = "8.8.8.8:443".parse::<SocketAddr>().unwrap();
        let dns_host = String::from("dns.google");
        let io_loop = Runtime::new().unwrap();
        let resolver: RecursiveResolver = (
            &Upstream::HttpsUpstream {
                address: vec![dns_addr],
                proxy: None,
                tls_host: dns_host,
            },
            &ResolverOpts::default(),
        )
            .into();
        let lookup_future = resolver.resolve("dns.google", RecordType::A);
        let response = io_loop.block_on(lookup_future).unwrap();
        assert!(response
            .record_iter()
            .any(|r| r.data().unwrap().to_string().eq("8.8.8.8")));
    }

    #[cfg(feature = "dns-over-h3")]
    #[test]
    fn h3_resolver_test() {
        let dns_addr = "8.8.8.8:443".parse::<SocketAddr>().unwrap();
        let dns_host = String::from("dns.google");
        let io_loop = Runtime::new().unwrap();
        let resolver: RecursiveResolver = (
            &Upstream::H3Upstream {
                address: vec![dns_addr],
                proxy: None,
                tls_host: dns_host,
            },
            &ResolverOpts::default(),
        )
            .into();
        let lookup_future = resolver.resolve("dns.google", RecordType::A);
        let response = io_loop.block_on(lookup_future).unwrap();
        assert!(response
            .record_iter()
            .any(|r| r.data().unwrap().to_string().eq("8.8.8.8")));
    }
}
