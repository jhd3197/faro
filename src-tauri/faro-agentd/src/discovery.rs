//! Best-effort mDNS advertising so Faro can discover this daemon on the LAN
//! without the user typing an IP. Publishes `_faro-agent._tcp` with the
//! machine's hostname, OS, and key fingerprint in TXT records. Discovery is a
//! convenience: pairing + pinning is what actually authenticates a connection,
//! so a failure here (locked-down network, no multicast) is non-fatal — the user
//! can still add the machine by IP:port.

use anyhow::{Context, Result};
use faro_agent_proto::msg::SERVICE_TYPE;
use mdns_sd::{ServiceDaemon, ServiceInfo};

/// Handle that keeps the mDNS registration alive; drop it to unpublish.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertisement {
    /// Announce this daemon on the LAN. `fingerprint` is the identity's short
    /// fingerprint; `os` the platform string; `port` the TCP port we serve on.
    pub fn publish(port: u16, fingerprint: &str, os: &str) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("start mDNS responder")?;
        let host = gethostname::gethostname().to_string_lossy().to_string();
        // Instance name is the hostname; keep it stable so re-announcing updates
        // rather than duplicates.
        let instance = sanitize(&host);
        let props = [
            ("fp".to_string(), fingerprint.to_string()),
            ("os".to_string(), os.to_string()),
            ("host".to_string(), host.clone()),
            ("v".to_string(), env!("CARGO_PKG_VERSION").to_string()),
        ];
        // Let mdns-sd resolve our addresses (empty host_ipv4 + auto-addr).
        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &instance,
            &format!("{instance}.local."),
            "",
            port,
            &props[..],
        )
        .context("build mDNS service info")?
        .enable_addr_auto();
        let fullname = service.get_fullname().to_string();
        daemon.register(service).context("register mDNS service")?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
    }
}

/// mDNS instance names can't contain dots; replace anything awkward.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}
