use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};

use super::*;

fn entry(outcome: McpOutcome) -> McpAuditEntry {
    McpAuditEntry {
        id: 7,
        timestamp: "2026-10-30T12:00:00.000Z".to_string(),
        tool: "query_table".to_string(),
        connection_id: Some(1),
        connection_name: "Prod".to_string(),
        arguments: "{}".to_string(),
        outcome,
        reason: "because".to_string(),
        row_count: None,
        bytes: 0,
        duration_ms: 5,
        client_name: "agent 1".to_string(),
        prev_hash: "p".to_string(),
        hash: "h".to_string(),
    }
}

#[test]
fn config_parsing_requires_a_target_where_one_is_needed() {
    assert_eq!(parse_config("", ""), SinkConfig::None);
    assert_eq!(parse_config("syslog", ""), SinkConfig::None);
    assert_eq!(parse_config("http", "  "), SinkConfig::None);
    assert_eq!(parse_config("bogus", "x"), SinkConfig::None);
    assert_eq!(
        parse_config("SysLog", " logs:514 "),
        SinkConfig::Syslog("logs:514".into())
    );
    assert_eq!(parse_config("eventlog", ""), SinkConfig::EventLog);
    assert_eq!(
        parse_config("http", "https://siem/ingest"),
        SinkConfig::Http("https://siem/ingest".into())
    );
}

#[test]
fn syslog_message_follows_rfc_5424_with_the_json_entry_as_payload() {
    let message = syslog_message(&entry(McpOutcome::Allowed));
    assert!(message.starts_with("<110>1 2026-10-30T12:00:00.000Z - schemetry-mcp "));
    assert!(message.contains(" query_table - {"));
    assert!(message.ends_with('}'));
    // Denied and failed calls go out at warning severity.
    assert!(syslog_message(&entry(McpOutcome::Denied)).starts_with("<108>1 "));
    assert!(syslog_message(&entry(McpOutcome::Error)).starts_with("<108>1 "));
}

#[test]
fn rendered_entries_round_trip_through_json() {
    let original = entry(McpOutcome::Denied);
    let parsed: McpAuditEntry = serde_json::from_str(&render(&original)).unwrap();
    assert_eq!(parsed, original);
}

#[test]
fn no_sink_does_nothing() {
    assert!(deliver(&SinkConfig::None, &entry(McpOutcome::Allowed)).is_ok());
}

#[test]
fn udp_syslog_delivers_to_a_listener() {
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let target = listener.local_addr().unwrap().to_string();

    deliver(&SinkConfig::Syslog(target), &entry(McpOutcome::Allowed)).unwrap();

    let mut buffer = [0u8; 4096];
    let received = listener.recv(&mut buffer).unwrap();
    let text = String::from_utf8_lossy(&buffer[..received]);
    assert!(text.contains("\"tool\":\"query_table\""));
}

#[test]
fn http_sink_posts_json() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/audit", server.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut received = Vec::new();
        let mut chunk = [0u8; 2048];
        // Read until the body (a JSON object) has fully arrived.
        while !received.ends_with(b"}") {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            received.extend_from_slice(&chunk[..n]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        String::from_utf8_lossy(&received).into_owned()
    });

    deliver(&SinkConfig::Http(url), &entry(McpOutcome::Allowed)).unwrap();

    let request = handle.join().unwrap();
    assert!(request.starts_with("POST /audit HTTP/1.1"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    assert!(request.contains("\"tool\":\"query_table\""));
}

#[test]
fn http_sink_rejects_other_schemes_and_reports_failures() {
    assert!(
        deliver(
            &SinkConfig::Http("ftp://x".into()),
            &entry(McpOutcome::Allowed)
        )
        .is_err()
    );
    // Nothing listens on this port.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", closed.local_addr().unwrap());
    drop(closed);
    assert!(deliver(&SinkConfig::Http(url), &entry(McpOutcome::Allowed)).is_err());
}

#[cfg(not(windows))]
#[test]
fn the_event_log_sink_is_windows_only() {
    assert!(deliver(&SinkConfig::EventLog, &entry(McpOutcome::Allowed)).is_err());
}
