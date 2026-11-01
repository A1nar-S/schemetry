//! Optional forwarding of each MCP audit entry to a sink outside Schemetry's control
//! (syslog, Windows Event Log or an HTTP endpoint), so a copy survives even if the local
//! SQLite file is rewritten. Configured through the `mcp_audit_sink_*` settings.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::models::{McpAuditEntry, McpOutcome};
use crate::services::mcp_audit_target;
use crate::services::settings;

const FORWARD_TIMEOUT: Duration = Duration::from_secs(3);
const SYSLOG_APP_NAME: &str = "schemetry-mcp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SinkConfig {
    None,
    /// `host[:port]` (UDP, default 514) or `local` for the Unix `/dev/log` socket.
    Syslog(String),
    EventLog,
    Http(String),
}

pub fn parse_config(kind: &str, target: &str) -> SinkConfig {
    let target = target.trim();
    match kind.trim().to_ascii_lowercase().as_str() {
        "syslog" if !target.is_empty() => SinkConfig::Syslog(target.to_string()),
        "eventlog" => SinkConfig::EventLog,
        "http" if !target.is_empty() => SinkConfig::Http(target.to_string()),
        _ => SinkConfig::None,
    }
}

pub fn load_config() -> SinkConfig {
    let (kind, target) = settings::load_mcp_audit_sink();
    parse_config(&kind, &target)
}

/// The entry as one JSON line — the payload every sink receives. `target` repeats the
/// human-readable target summary at the top level (the structured form is inside
/// `arguments`) so receivers need not parse the arguments to see what was touched.
pub fn render(entry: &McpAuditEntry) -> String {
    let Ok(mut value) = serde_json::to_value(entry) else {
        return String::new();
    };
    if let Some(map) = value.as_object_mut() {
        map.insert(
            "target".to_string(),
            serde_json::Value::String(mcp_audit_target::summarize(&entry.arguments)),
        );
    }
    value.to_string()
}

pub fn forward(entry: &McpAuditEntry) -> Result<()> {
    deliver(&load_config(), entry)
}

pub fn deliver(config: &SinkConfig, entry: &McpAuditEntry) -> Result<()> {
    match config {
        SinkConfig::None => Ok(()),
        SinkConfig::Syslog(target) => send_syslog(target, entry),
        SinkConfig::EventLog => send_event_log(entry),
        SinkConfig::Http(url) => send_http(url, entry),
    }
}

/// RFC 5424 message: facility 13 (log audit), warning severity for denied/failed calls.
pub fn syslog_message(entry: &McpAuditEntry) -> String {
    let severity = if entry.outcome == McpOutcome::Allowed {
        6
    } else {
        4
    };
    let priority = 13 * 8 + severity;
    format!(
        "<{priority}>1 {} - {SYSLOG_APP_NAME} {} {} - {}",
        entry.timestamp,
        std::process::id(),
        entry.tool,
        render(entry)
    )
}

fn send_syslog(target: &str, entry: &McpAuditEntry) -> Result<()> {
    let message = syslog_message(entry);

    if target.eq_ignore_ascii_case("local") {
        #[cfg(unix)]
        {
            let socket = std::os::unix::net::UnixDatagram::unbound()?;
            socket
                .send_to(message.as_bytes(), "/dev/log")
                .context("Could not write to /dev/log")?;
            return Ok(());
        }
        #[cfg(not(unix))]
        bail!("Local syslog is only available on Unix; use host:port instead");
    }

    use std::net::{ToSocketAddrs, UdpSocket};
    let with_port = if target.contains(':') {
        target.to_string()
    } else {
        format!("{target}:514")
    };
    let address = with_port
        .to_socket_addrs()
        .with_context(|| format!("Could not resolve syslog target '{target}'"))?
        .next()
        .ok_or_else(|| anyhow!("Syslog target '{target}' did not resolve"))?;
    let bind = if address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind)?;
    socket.set_write_timeout(Some(FORWARD_TIMEOUT))?;
    socket.send_to(message.as_bytes(), address)?;
    Ok(())
}

fn send_http(url: &str, entry: &McpAuditEntry) -> Result<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("HTTP audit sink must be an http:// or https:// URL");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FORWARD_TIMEOUT))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(render(entry))
        .with_context(|| format!("Could not POST audit entry to {url}"))?;
    Ok(())
}

#[cfg(windows)]
fn send_event_log(entry: &McpAuditEntry) -> Result<()> {
    use windows_sys::Win32::System::EventLog::{
        DeregisterEventSource, EVENTLOG_INFORMATION_TYPE, EVENTLOG_WARNING_TYPE,
        RegisterEventSourceW, ReportEventW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let source = wide("Schemetry");
    let message = wide(&render(entry));
    let event_type = if entry.outcome == McpOutcome::Allowed {
        EVENTLOG_INFORMATION_TYPE
    } else {
        EVENTLOG_WARNING_TYPE
    };

    // SAFETY: both buffers are NUL-terminated and outlive the calls; the handle is closed.
    unsafe {
        let handle = RegisterEventSourceW(std::ptr::null(), source.as_ptr());
        if handle.is_null() {
            bail!("Could not open the Windows Event Log");
        }
        let strings = [message.as_ptr()];
        let ok = ReportEventW(
            handle,
            event_type,
            0,
            1,
            std::ptr::null_mut(),
            1,
            0,
            strings.as_ptr(),
            std::ptr::null(),
        );
        DeregisterEventSource(handle);
        if ok == 0 {
            bail!("Could not write to the Windows Event Log");
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn send_event_log(_entry: &McpAuditEntry) -> Result<()> {
    bail!("The Windows Event Log sink is only available on Windows")
}

#[cfg(test)]
#[path = "tests/mcp_audit_sink.rs"]
mod tests;
