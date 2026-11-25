//! KOSync and CrossPoint Sync progress synchronization.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::config::{DocumentId, SyncConfig};

/// Media type expected by KOReader sync servers.
const ACCEPT: &str = "application/vnd.koreader.v1+json";
/// Upper bound for one sync request, so a dead server cannot hang the reader.
const TIMEOUT: Duration = Duration::from_secs(3);

/// KOReader samples 1 KiB at these byte offsets for its partial MD5.
///
/// The `i = -1` term of `util.partialMD5` evaluates to offset 0.
const SAMPLE_OFFSETS: [u64; 12] = [
    0,
    1_024,
    4_096,
    16_384,
    65_536,
    262_144,
    1_048_576,
    4_194_304,
    16_777_216,
    67_108_864,
    268_435_456,
    1_073_741_824,
];

/// A remote reading position.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteProgress {
    /// Whole-book progress, from 0 to 1.
    pub percentage: f32,
    /// Server write time in unix seconds.
    pub timestamp: u64,
    /// Device that wrote the position.
    pub device: String,
}

/// An authenticated KOSync client.
pub struct SyncClient {
    base: String,
    username: String,
    key: String,
    device: String,
    device_id: String,
    agent: ureq::Agent,
}

impl SyncClient {
    /// Build a client from the `[sync]` configuration.
    pub fn new(config: &SyncConfig) -> Result<Self> {
        config.validate()?;
        let key = if config.key.trim().is_empty() {
            md5_hex(config.password.as_bytes())
        } else {
            config.key.trim().to_ascii_lowercase()
        };
        let base = config.server.trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            bail!("sync.server must not be empty");
        }
        let device = if config.device.trim().is_empty() {
            "slurp".to_string()
        } else {
            config.device.trim().to_string()
        };
        let device_id = if config.device_id.trim().is_empty() {
            "slurp".to_string()
        } else {
            config.device_id.trim().to_string()
        };
        let agent = ureq::AgentBuilder::new()
            .user_agent(concat!("slurp/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(TIMEOUT)
            .timeout_read(TIMEOUT)
            .timeout_write(TIMEOUT)
            .build();
        Ok(Self {
            base,
            username: config.username.clone(),
            key,
            device,
            device_id,
            agent,
        })
    }

    /// Fetch the newest position for a document.
    ///
    /// Returns `None` when the server has no position for it.
    pub fn pull(&self, document: &str) -> Result<Option<RemoteProgress>> {
        let url = format!("{}/syncs/progress/{document}", self.base);
        let response = self
            .agent
            .get(&url)
            .timeout(TIMEOUT)
            .set("Accept", ACCEPT)
            .set("x-auth-user", &self.username)
            .set("x-auth-key", &self.key)
            .call();
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(401, _)) => {
                bail!("sync server rejected the login");
            }
            Err(ureq::Error::Status(404, _))
            | Err(ureq::Error::Status(204, _)) => return Ok(None),
            Err(ureq::Error::Status(code, response)) => bail!(
                "sync server returned {code} {}",
                response.status_text()
            ),
            Err(error) => {
                return Err(error).context("cannot reach the sync server");
            }
        };
        let body: ProgressBody =
            response.into_json().context("invalid sync response")?;
        let Some(percentage) = body.percentage else {
            return Ok(None);
        };
        Ok(Some(RemoteProgress {
            percentage: percentage.clamp(0.0, 1.0) as f32,
            timestamp: body.timestamp.unwrap_or(0),
            device: body.device.unwrap_or_default(),
        }))
    }

    /// Store the local position.
    pub fn push(
        &self,
        document: &str,
        percentage: f32,
        title: &str,
        author: Option<&str>,
        filename: &str,
    ) -> Result<()> {
        let percentage = percentage.clamp(0.0, 1.0);
        let mut metadata = Map::new();
        if !filename.is_empty() {
            metadata.insert("filename".into(), Value::from(filename));
        }
        if !title.is_empty() {
            metadata.insert("title".into(), Value::from(title));
        }
        if let Some(author) = author.filter(|author| !author.is_empty()) {
            metadata.insert("authors".into(), Value::from(author));
        }
        let mut body = json!({
            "document": document,
            // Not a KOReader XPointer or page number: the percentage is the
            // portable part. A decimal would be read as a page by KOReader.
            "progress": format!("{:.2}%", percentage * 100.0),
            "percentage": percentage,
            "device": self.device,
            "device_id": self.device_id,
        });
        if !metadata.is_empty() {
            body["metadata"] = Value::Object(metadata);
        }

        let url = format!("{}/syncs/progress", self.base);
        let response = self
            .agent
            .put(&url)
            .timeout(TIMEOUT)
            .set("Accept", ACCEPT)
            .set("x-auth-user", &self.username)
            .set("x-auth-key", &self.key)
            .send_json(&body);
        match response {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(401, _)) => {
                bail!("sync server rejected the login");
            }
            Err(ureq::Error::Status(code, response)) => bail!(
                "sync server returned {code} {}",
                response.status_text()
            ),
            Err(error) => Err(error).context("cannot reach the sync server"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProgressBody {
    #[serde(default)]
    percentage: Option<f64>,
    #[serde(default)]
    timestamp: Option<u64>,
    #[serde(default)]
    device: Option<String>,
}

/// Document identifier used by KOReader and CrossPoint for an EPUB.
pub fn document_id(path: &Path, mode: DocumentId) -> Result<String> {
    match mode {
        DocumentId::Binary => partial_md5(path),
        DocumentId::Filename => {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .with_context(|| {
                    format!("cannot read the file name of {}", path.display())
                })?;
            Ok(md5_hex(name.as_bytes()))
        }
    }
}

/// KOReader's partial MD5 of a file.
fn partial_md5(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let size = file
        .metadata()
        .with_context(|| format!("cannot stat {}", path.display()))?
        .len();
    let mut hasher = Md5::new();
    let mut buffer = [0u8; 1024];
    for offset in SAMPLE_OFFSETS {
        if offset >= size {
            break;
        }
        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("cannot seek in {}", path.display()))?;
        let length = (size - offset).min(1024) as usize;
        file.read_exact(&mut buffer[..length])
            .with_context(|| format!("cannot read {}", path.display()))?;
        hasher.update(&buffer[..length]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", Md5::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    struct Request {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    fn serve(
        status: &'static str,
        body: &'static str,
    ) -> (String, mpsc::Receiver<Request>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let mut headers = Vec::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    let name = name.trim().to_ascii_lowercase();
                    if name == "content-length" {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    headers.push((name, value.trim().to_string()));
                }
            }
            let mut request_body = vec![0u8; length];
            reader.read_exact(&mut request_body).unwrap();
            sender
                .send(Request {
                    method,
                    path,
                    headers,
                    body: String::from_utf8(request_body).unwrap(),
                })
                .unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}"), receiver, handle)
    }

    fn client(server: &str) -> SyncClient {
        SyncClient::new(&SyncConfig {
            enabled: true,
            server: server.to_string(),
            username: "reader".to_string(),
            password: "secret".to_string(),
            ..SyncConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn computes_koreader_partial_md5() {
        let digest = partial_md5(Path::new("tests/sample.epub")).unwrap();
        assert_eq!(digest, "2babf939094150df4dd24c53baf2b89a");
    }

    #[test]
    fn computes_filename_md5() {
        let digest = document_id(
            Path::new("/books/Example Book.epub"),
            DocumentId::Filename,
        )
        .unwrap();
        assert_eq!(digest, "662978afc7ed684283cc2cb3e56bd604");
    }

    #[test]
    fn pulls_a_position() {
        let (server, requests, handle) = serve(
            "200 OK",
            r#"{"percentage":0.42,"timestamp":123,"device":"CrossPoint"}"#,
        );
        let remote = client(&server).pull("abc123").unwrap().unwrap();
        assert!((remote.percentage - 0.42).abs() < 0.0001);
        assert_eq!(remote.timestamp, 123);
        assert_eq!(remote.device, "CrossPoint");

        let request = requests.recv().unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/syncs/progress/abc123");
        assert!(request.headers.contains(&(
            "x-auth-user".to_string(),
            "reader".to_string()
        )));
        assert!(request.headers.contains(&(
            "x-auth-key".to_string(),
            "5ebe2294ecd0e0f08eab7690d2a6ee69".to_string()
        )));
        handle.join().unwrap();
    }

    #[test]
    fn treats_an_empty_answer_as_no_progress() {
        let (server, _requests, handle) = serve("200 OK", "{}");
        assert_eq!(client(&server).pull("abc123").unwrap(), None);
        handle.join().unwrap();
    }

    #[test]
    fn reports_rejected_credentials() {
        let (server, _requests, handle) =
            serve("401 Unauthorized", r#"{"code":2001}"#);
        let error = client(&server).pull("abc123").unwrap_err();
        assert!(error.to_string().contains("rejected"), "{error:#}");
        handle.join().unwrap();
    }

    #[test]
    fn pushes_progress() {
        let (server, requests, handle) = serve(
            "200 OK",
            r#"{"document":"abc123","timestamp":123}"#,
        );
        client(&server)
            .push(
                "abc123",
                0.625,
                "A Book",
                Some("An Author"),
                "A Book.epub",
            )
            .unwrap();

        let request = requests.recv().unwrap();
        assert_eq!(request.method, "PUT");
        assert_eq!(request.path, "/syncs/progress");
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["document"], "abc123");
        assert_eq!(body["percentage"], 0.625);
        assert_eq!(body["progress"], "62.50%");
        assert_eq!(body["device"], "slurp");
        assert_eq!(body["device_id"], "slurp");
        assert_eq!(body["metadata"]["title"], "A Book");
        assert_eq!(body["metadata"]["authors"], "An Author");
        handle.join().unwrap();
    }
}
