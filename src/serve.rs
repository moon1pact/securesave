use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::SystemTime;

use securesave::{Api, Compression, JobStatus};
use tiny_http::{Header, Response, Server};

pub fn run(api: &Api, port: u16) -> ExitCode {
    let addr = format!("127.0.0.1:{port}");
    let server = match Server::http(&addr) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("securesave: error: cannot listen on {addr}: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("Serving on http://{addr} (Ctrl-C to stop)");

    for request in server.incoming_requests() {
        let response = match request.url() {
            "/" => match api.status() {
                Ok(statuses) => html(render_status_page(&statuses, SystemTime::now()), 200),
                Err(err) => html(render_error_page(&err.to_string()), 500),
            },
            _ => html("<h1>404</h1>".to_string(), 404),
        };
        if let Err(e) = request.respond(response) {
            eprintln!("securesave: warning: failed to send a response: {e}");
        }
    }
    ExitCode::SUCCESS
}

fn html(body: String, status: u16) -> Response<std::io::Cursor<Vec<u8>>> {
    let content_type = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
        .expect("static header is valid");
    Response::from_string(body)
        .with_status_code(status)
        .with_header(content_type)
}

fn render_status_page(statuses: &[JobStatus], now: SystemTime) -> String {
    let mut rows = String::new();
    for status in statuses {
        let compression = match status.compression {
            Compression::None => "none",
            Compression::Zstd => "zstd",
        };
        let state = if status.destination_exists {
            r#"<span class="ok">OK</span>"#.to_string()
        } else {
            r#"<span class="bad">destination missing</span>"#.to_string()
        };
        let last_run = match status.last_run {
            Some(time) => escape(&crate::ago(time, now)),
            None => "unknown".to_string(),
        };
        let files = status
            .files_recorded
            .map_or("-".to_string(), |n| n.to_string());
        let _ = write!(
            rows,
            "<tr><td>{}</td><td><code>{}</code></td><td><code>{}</code></td>\
             <td>{compression}</td><td>{state}</td><td>{last_run}</td><td>{files}</td></tr>",
            escape(&status.name),
            escape(&status.source.display().to_string()),
            escape(&status.destination.display().to_string()),
        );
    }
    let table = if statuses.is_empty() {
        "<p>No jobs defined in the configuration file.</p>".to_string()
    } else {
        format!(
            "<table><tr><th>Job</th><th>Source</th><th>Destination</th>\
             <th>Compression</th><th>State</th><th>Last run</th><th>Files</th></tr>{rows}</table>"
        )
    };
    page("SecureSave", &table)
}

fn render_error_page(message: &str) -> String {
    page(
        "SecureSave - error",
        &format!(r#"<p class="bad">{}</p>"#, escape(message)),
    )
}

fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta http-equiv="refresh" content="10">
<title>{title}</title>
<style>
body {{ font-family: sans-serif; margin: 2rem; }}
table {{ border-collapse: collapse; }}
td, th {{ border: 1px solid #ccc; padding: 0.4rem 0.8rem; text-align: left; }}
code {{ font-size: 0.9em; }}
.ok {{ color: #1a7f37; }} .bad {{ color: #b91c1c; }}
</style></head>
<body><h1>{title}</h1>{body}</body></html>
"#
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(
            escape(r#"<b>&"x"</b>"#),
            "&lt;b&gt;&amp;&quot;x&quot;&lt;/b&gt;"
        );
    }

    #[test]
    fn renders_jobs_with_escaped_names() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let statuses = vec![JobStatus {
            name: "<script>".to_string(),
            source: PathBuf::from("/s"),
            destination: PathBuf::from("/d"),
            compression: Compression::Zstd,
            destination_exists: true,
            last_run: Some(now - Duration::from_secs(120)),
            files_recorded: Some(42),
        }];

        let html = render_status_page(&statuses, now);

        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("2 minute(s) ago"));
        assert!(html.contains("<td>42</td>"));
    }

    #[test]
    fn renders_a_message_when_no_jobs_are_defined() {
        let html = render_status_page(&[], SystemTime::now());
        assert!(html.contains("No jobs defined"));
    }
}
