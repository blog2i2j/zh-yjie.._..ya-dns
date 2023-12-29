use crate::ip::IpRange;
use hickory_proto::rr::RecordType;
use ipnet::AddrParseError;
use ipnet::IpNet;
use serde_derive::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::str::FromStr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("You must configure at least one default upstream server!")]
    NoUpstream,
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[cfg(any(feature = "dns-over-tls", feature = "dns-over-https"))]
    #[error("tls-host is missing")]
    NoTlsHost,
}

#[derive(Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub default_upstreams: Vec<String>,
    pub upstreams: HashMap<String, Upstream>,
    pub domains: HashMap<String, Domains>,
    pub ranges: HashMap<String, IpRange>,
    pub request_rules: Vec<RequestRule>,
    pub response_rules: Vec<ResponseRule>,
}

#[derive(Debug, Deserialize)]
pub struct ConfigBuilder {
    bind: SocketAddr,
    upstreams: HashMap<String, UpstreamConfig>,
    domains: Option<HashMap<String, DomainsConf>>,
    ranges: Option<HashMap<String, IpRangeConf>>,
    requests: Option<Vec<RequestRuleConfig>>,
    responses: Option<Vec<ResponseRule>>,
}

#[derive(Debug)]
pub enum Upstream {
    UdpUpstream {
        address: SocketAddr,
    },
    TcpUpstream {
        address: SocketAddr,
        proxy: Option<String>,
    },
    #[cfg(feature = "dns-over-tls")]
    TlsUpstream {
        address: SocketAddr,
        tls_host: String,
        proxy: Option<String>,
    },
    #[cfg(feature = "dns-over-https")]
    HttpsUpstream {
        address: SocketAddr,
        tls_host: String,
        proxy: Option<String>,
    },
}

impl ConfigBuilder {
    pub fn build(self) -> Result<Config, ConfigError> {
        let mut default_upstreams = Vec::new();

        let upstreams = self
            .upstreams
            .into_iter()
            .map(|(key, upstream)| {
                if upstream.default {
                    default_upstreams.push(key.clone())
                }
                upstream.build().map(move |upstream| (key, upstream))
            })
            .collect::<Result<HashMap<_, _>, ConfigError>>()?;

        if default_upstreams.is_empty() {
            return Err(ConfigError::NoUpstream);
        }

        let domains = self
            .domains
            .unwrap_or_default()
            .into_iter()
            .map(|(key, domains)| domains.build().map(move |domains| (key, domains)))
            .collect::<Result<HashMap<_, _>, ConfigError>>()?;

        let ranges = self
            .ranges
            .unwrap_or_default()
            .into_iter()
            .map(|(key, conf)| {
                let mut range = IpRange::new();
                conf.read_to(&mut range).map(|()| (key, range))
            })
            .collect::<Result<HashMap<_, _>, AddrParseError>>()
            .unwrap();

        let request_rules: Vec<RequestRule> = self
            .requests
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.build())
            .collect::<Result<Vec<_>, ConfigError>>()?;

        Ok(Config {
            bind: self.bind,
            default_upstreams,
            upstreams,
            domains,
            ranges,
            request_rules,
            response_rules: self.responses.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct UpstreamConfig {
    address: String,
    network: NetworkType,
    proxy: Option<String>,
    #[cfg(any(feature = "dns-over-tls", feature = "dns-over-https"))]
    #[serde(rename = "tls-host")]
    tls_host: Option<String>,
    #[serde(default = "UpstreamConfig::default_default")]
    default: bool,
}

impl UpstreamConfig {
    fn default_default() -> bool {
        true
    }

    fn build(self) -> Result<Upstream, ConfigError> {
        let mut address = self.address.parse::<SocketAddr>();
        if let Err(_) = address {
            address = self
                .address
                .parse::<IpAddr>()
                .map(|addr| SocketAddr::new(addr, self.network.default_port()));
        }
        let address = address.map_err(|_| ConfigError::InvalidAddress(self.address))?;
        let proxy = self.proxy;
        match self.network {
            NetworkType::Tcp => Ok(Upstream::TcpUpstream { address, proxy }),
            NetworkType::Udp => Ok(Upstream::UdpUpstream { address }),
            #[cfg(feature = "dns-over-tls")]
            NetworkType::Tls => {
                let tls_host = self.tls_host.ok_or(ConfigError::NoTlsHost)?;
                Ok(Upstream::TlsUpstream {
                    address,
                    tls_host,
                    proxy,
                })
            }
            #[cfg(feature = "dns-over-https")]
            NetworkType::Https => {
                let tls_host = self.tls_host.ok_or(ConfigError::NoTlsHost)?;
                Ok(Upstream::HttpsUpstream {
                    address,
                    tls_host,
                    proxy,
                })
            }
        }
    }
}

#[derive(Debug, Deserialize)]
enum NetworkType {
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "udp")]
    Udp,
    #[cfg(feature = "dns-over-tls")]
    #[serde(rename = "tls")]
    Tls,
    #[cfg(feature = "dns-over-https")]
    #[serde(rename = "https")]
    Https,
}

impl NetworkType {
    fn default_port(&self) -> u16 {
        match self {
            NetworkType::Tcp | NetworkType::Udp => 53,
            #[cfg(feature = "dns-over-tls")]
            NetworkType::Tls => 853,
            #[cfg(feature = "dns-over-https")]
            NetworkType::Https => 443,
        }
    }
}

#[derive(Debug, Deserialize)]
struct IpRangeConf {
    files: Option<Vec<String>>,
    list: Option<Vec<String>>,
}

impl IpRangeConf {
    fn read_to(&self, range: &mut IpRange) -> Result<(), AddrParseError> {
        if let Some(files) = &self.files {
            for file in files {
                let file = File::open(file).unwrap();
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    let line = line.unwrap();
                    let line = line.trim();
                    if line.is_empty() || line.starts_with("#") {
                        continue;
                    }
                    let ip_net: IpNet = line.parse()?;
                    range.add(ip_net);
                }
            }
        }

        if let Some(list) = &self.list {
            for ip_net in list {
                let ip_net: IpNet = ip_net.trim().parse()?;
                range.add(ip_net);
            }
        }

        range.simplify();
        Ok(())
    }
}

#[derive(Debug)]
pub struct Domains {
    pub regex_set: Vec<String>,
    pub suffix_set: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DomainsConf {
    files: Option<Vec<String>>,
    list: Option<Vec<String>>,
}

impl DomainsConf {
    fn build(self) -> Result<Domains, ConfigError> {
        let mut regex_set = Vec::new();
        let mut suffix_set = Vec::new();
        suffix_set.push(String::from("// BEGIN ICANN DOMAINS"));

        let mut push = |line: &str| {
            if line.is_empty() || line.starts_with("#") {
                return;
            }
            if line.starts_with("regexp:") {
                let line1 = line.trim_start_matches("regexp:");
                regex_set.push(String::from(line1));
            } else {
                let line1 = line.trim_start_matches("full:").trim_start_matches(".");
                suffix_set.push(String::from(line1));
            }
        };

        if let Some(files) = &self.files {
            for file in files {
                let file = File::open(file).unwrap();
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    let line = line.unwrap();
                    let line = line.trim();
                    push(line);
                }
            }
        }

        if let Some(list) = &self.list {
            for line in list {
                push(line);
            }
        }

        Ok(Domains {
            regex_set,
            suffix_set,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RequestRuleConfig {
    domains: Option<Vec<String>>,
    types: Option<Vec<String>>,
    upstreams: Vec<String>,
}

impl RequestRuleConfig {
    fn build(self) -> Result<RequestRule, ConfigError> {
        let types = Transpose::transpose(self.types.map(|v| {
            v.iter()
                .map(|t| RecordType::from_str(t))
                .collect::<Result<Vec<_>, _>>()
        }))
        .unwrap();

        Ok(RequestRule {
            domains: self.domains,
            types,
            upstreams: self.upstreams,
        })
    }
}

#[derive(Debug)]
pub struct RequestRule {
    pub domains: Option<Vec<String>>,
    pub types: Option<Vec<RecordType>>,
    pub upstreams: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResponseRule {
    pub upstreams: Option<Vec<String>>,
    pub ranges: Option<Vec<String>>,
    pub domains: Option<Vec<String>>,
    pub action: RuleAction,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub enum RuleAction {
    #[serde(rename = "accept")]
    Accept,
    #[serde(rename = "drop")]
    Drop,
}

trait Transpose {
    type Output;
    fn transpose(self) -> Self::Output;
}

impl<T, E> Transpose for Option<Result<T, E>> {
    type Output = Result<Option<T>, E>;

    fn transpose(self) -> Self::Output {
        match self {
            Some(Ok(x)) => Ok(Some(x)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}
