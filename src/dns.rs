use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use hickory_resolver::config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts};
use hickory_resolver::error::ResolveErrorKind;
use hickory_resolver::proto::op::ResponseCode;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::TokioAsyncResolver;

const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum QueryResult {
    /// Record values (rdata strings) and the minimum TTL seen.
    Records { values: Vec<String>, min_ttl: u32 },
    /// The server answered that the record does not exist (NXDOMAIN or
    /// NOERROR with an empty answer section). This is a real propagation
    /// signal — the server's view is "nothing there" — so it counts toward
    /// the responding total.
    NoRecords(String),
    /// No usable answer: timeout, network error, or the server refused to
    /// serve us (REFUSED/SERVFAIL). Says nothing about propagation, so these
    /// are excluded from the percentage.
    Error(String),
}

#[derive(Debug)]
pub struct QueryOutcome {
    pub resolver_index: usize,
    pub generation: u64,
    pub result: QueryResult,
    pub elapsed: Duration,
}

/// Query a single upstream resolver directly (no cache, single attempt) so
/// each server's own view of the record is what we measure.
pub async fn query(server: IpAddr, domain: String, rtype: RecordType) -> (QueryResult, Duration) {
    let mut config = ResolverConfig::new();
    let addr = SocketAddr::new(server, 53);
    config.add_name_server(NameServerConfig::new(addr, Protocol::Udp));
    // TCP entry lets hickory retry there when a UDP answer comes back
    // truncated (large TXT sets, long MX lists, …).
    config.add_name_server(NameServerConfig::new(addr, Protocol::Tcp));

    let mut opts = ResolverOpts::default();
    opts.timeout = QUERY_TIMEOUT;
    opts.attempts = 1;
    opts.cache_size = 0;
    opts.use_hosts_file = false;
    opts.edns0 = true; // allow >512-byte UDP answers

    let resolver = TokioAsyncResolver::tokio(config, opts);

    let start = Instant::now();
    let lookup = tokio::time::timeout(QUERY_TIMEOUT + Duration::from_secs(1), resolver.lookup(domain.as_str(), rtype)).await;
    let elapsed = start.elapsed();

    let result = match lookup {
        Err(_) => QueryResult::Error("timeout".into()),
        Ok(Err(err)) => match err.kind() {
            ResolveErrorKind::NoRecordsFound { response_code, .. } => match response_code {
                // "Won't serve you" / "couldn't resolve" — not a statement
                // about whether the record exists.
                ResponseCode::Refused => QueryResult::Error("refused".into()),
                ResponseCode::ServFail => QueryResult::Error("SERVFAIL".into()),
                code => QueryResult::NoRecords(code.to_string()),
            },
            ResolveErrorKind::Timeout => QueryResult::Error("timeout".into()),
            other => QueryResult::Error(short_error(&other.to_string())),
        },
        Ok(Ok(lookup)) => {
            let mut values: Vec<String> = Vec::new();
            let mut min_ttl = u32::MAX;
            for record in lookup.record_iter() {
                let Some(data) = record.data() else { continue };
                min_ttl = min_ttl.min(record.ttl());
                // A lookup can carry other types too (e.g. the CNAME hops on
                // the way to an A record); label those so answers stay
                // comparable across resolvers.
                if record.record_type() == rtype {
                    values.push(data.to_string());
                } else {
                    values.push(format!("{} {}", record.record_type(), data));
                }
            }
            values.sort();
            values.dedup();
            if values.is_empty() {
                QueryResult::NoRecords("empty answer".into())
            } else {
                QueryResult::Records { values, min_ttl }
            }
        }
    };

    (result, elapsed)
}

fn short_error(message: &str) -> String {
    let msg = message.to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        "timeout".into()
    } else if msg.contains("refused") {
        "refused".into()
    } else {
        let mut m = message.to_string();
        m.truncate(48);
        m
    }
}
