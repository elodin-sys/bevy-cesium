use crate::ion::{Grant, Ion};
use crate::{Completion, Error, Payload, Result, Tile, content, parse_root};
use crate::{HttpError, HttpSource};
use glam::DMat4;
use std::sync::{Arc, Mutex};
use std::{sync::mpsc::SyncSender, time::Duration};
use url::Url;

#[derive(Clone)]
pub(crate) struct Loader(
    ureq::Agent,
    Arc<Mutex<Option<Ion>>>,
    Arc<Mutex<Vec<String>>>,
    Arc<Option<crate::terrain::Imagery>>,
    Arc<Mutex<Option<crate::package::Package>>>,
    Arc<Mutex<()>>, // Provider authorization only; local reads never acquire it.
    Arc<Mutex<crate::retry::Cooldowns>>,
);
impl Default for Loader {
    fn default() -> Self {
        Self(
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(20)))
                .build()
                .into(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
        )
    }
}
impl Loader {
    fn request(
        &self,
        url: &Url,
        token: Option<&str>,
        strict: bool,
        source: HttpSource,
    ) -> Result<Vec<u8>> {
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
        };
        self.6
            .lock()
            .map_err(|_| Error::Message("Retry state unavailable"))?
            .check(source, now())?;
        let result =
            fetch_restricted(&self.0, url, token, strict).map_err(|e| e.with_source(source));
        if let Err(error) = result {
            self.6
                .lock()
                .map_err(|_| Error::Message("Retry state unavailable"))?
                .observe(error, now());
        }
        result
    }

    pub(crate) fn requires_provider(&self, url: &Url) -> bool {
        self.4.lock().ok().is_some_and(|state| {
            state.as_ref().is_some_and(|package| {
                matches!(
                    package.request(url),
                    Ok(crate::package::Request::Provider(_))
                )
            })
        })
    }

    pub(crate) fn prepared(package: crate::package::Package) -> Self {
        Self {
            4: Arc::new(Mutex::new(Some(package))),
            ..Self::default()
        }
    }
    fn prepared_root(&self, url: Url) -> Result<Url> {
        let manifest_url = self
            .4
            .lock()
            .map_err(|_| Error::Message("Package unavailable"))?
            .as_ref()
            .filter(|p| p.token.is_some())
            .map(|p| p.local("package.json"))
            .transpose()?;
        let Some(manifest_url) = manifest_url else {
            return Ok(url);
        };
        let bytes = self.request(&manifest_url, None, true, HttpSource::Package)?;
        self.4
            .lock()
            .map_err(|_| Error::Message("Package unavailable"))?
            .as_mut()
            .ok_or(Error::Message("Package unavailable"))?
            .install(&bytes)
    }
    fn provider_url(&self, url: &Url, refresh: bool) -> Result<Url> {
        // Share one grant between provider misses without holding the package
        // lock over network I/O: cached resources must remain readable meanwhile.
        let _authorization = self
            .5
            .lock()
            .map_err(|_| Error::Message("Provider unavailable"))?;
        let ion = {
            let state = self
                .4
                .lock()
                .map_err(|_| Error::Message("Package unavailable"))?;
            let package = state
                .as_ref()
                .ok_or(Error::Message("Package unavailable"))?;
            if !refresh && let Some(credentials) = &package.credentials {
                return credentials.authorize(url);
            }
            let manifest = package
                .manifest
                .as_ref()
                .ok_or(Error::Message("Missing package manifest"))?;
            Ion::new(
                manifest.ion_asset_id,
                package
                    .token
                    .as_deref()
                    .ok_or(Error::Message("Offline terrain"))?,
            )?
        };
        let grant = Grant::parse(&self.request(
            &ion.endpoint,
            Some(&ion.token),
            false,
            HttpSource::Provider,
        )?)?;
        crate::package::canonical(&grant.url)?;
        let bytes = self.request(&grant.url, None, true, HttpSource::Provider)?;
        let credentials = crate::package::GoogleSession::from_root(&grant.url, &bytes)?;
        let authorized = credentials.authorize(url)?;
        self.4
            .lock()
            .map_err(|_| Error::Message("Package unavailable"))?
            .as_mut()
            .ok_or(Error::Message("Package unavailable"))?
            .credentials = Some(credentials);
        self.2
            .lock()
            .map_err(|_| Error::Message("Credits unavailable"))?
            .extend(grant.credits);
        Ok(authorized)
    }

    pub(crate) fn with_imagery(mut self, imagery: Option<crate::terrain::Imagery>) -> Self {
        self.3 = Arc::new(imagery);
        self
    }
    pub(crate) fn ion(ion: Ion) -> Self {
        Self(
            Self::default().0,
            Arc::new(Mutex::new(Some(ion))),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
        )
    }
    pub(crate) fn credits(&self) -> Vec<String> {
        let mut credits = self
            .2
            .lock()
            .map(|credits| credits.clone())
            .unwrap_or_default();
        if let Some(imagery) = self.3.as_ref() {
            credits.push(imagery.credit.clone());
        }
        credits
    }
    fn root_url(&self, url: Option<Url>) -> Result<Url> {
        if let Some(url) = url {
            return Ok(url);
        }
        let mut state = self
            .1
            .lock()
            .map_err(|_| Error::Message("Ion state unavailable"))?;
        let ion = state
            .as_mut()
            .ok_or(Error::Message("Missing ion configuration"))?;
        ion.grant = Some(Grant::parse(&fetch_authorized(
            &self.0,
            &ion.endpoint,
            Some(&ion.token),
        )?)?);
        let grant = ion.grant.as_ref().unwrap();
        *self
            .2
            .lock()
            .map_err(|_| Error::Message("Ion credits unavailable"))? = grant.credits.clone();
        Ok(grant.url.clone())
    }
    fn fetch(&self, url: &Url) -> Result<Vec<u8>> {
        let request = self
            .4
            .lock()
            .map_err(|_| Error::Message("Package unavailable"))?
            .as_ref()
            .map(|p| p.request(url))
            .transpose()?;
        if let Some(request) = request {
            let remote = match request {
                crate::package::Request::Local(local) => {
                    return self.request(&local, None, true, HttpSource::Package);
                }
                crate::package::Request::Cached { local, remote } => {
                    match self.request(&local, None, true, HttpSource::Package) {
                        Ok(bytes) => return Ok(bytes),
                        Err(e)
                            if e.status() == Some(404) || e == Error::Message("File not found") =>
                        {
                            remote
                        }
                        Err(error) => return Err(error),
                    }
                }
                crate::package::Request::Provider(remote) => remote,
            };
            let authorized = self
                .provider_url(&remote, false)
                .map_err(|e| e.with_source(HttpSource::Provider))?;
            let result = self.request(&authorized, None, true, HttpSource::Provider);
            if matches!(
                result.as_ref().err().and_then(|e| e.status()),
                Some(401 | 403)
            ) {
                let refreshed = self
                    .provider_url(&remote, true)
                    .map_err(|e| e.with_source(HttpSource::Provider))?;
                return self.request(&refreshed, None, true, HttpSource::Provider);
            } else {
                return result;
            }
        }

        let token = {
            let state = self
                .1
                .lock()
                .map_err(|_| Error::Message("Ion state unavailable"))?;
            state
                .as_ref()
                .and_then(|i| i.grant.as_ref())
                .filter(|g| g.authorizes(url))
                .map(|g| g.token.clone())
        };
        let result = self.request(url, token.as_deref(), false, HttpSource::Source);
        if result.as_ref().err().and_then(|e| e.status()) != Some(401) || token.is_none() {
            return result;
        }
        let new_token = {
            let mut state = self
                .1
                .lock()
                .map_err(|_| Error::Message("Ion state unavailable"))?;
            let ion = state
                .as_mut()
                .ok_or(Error::Message("Missing ion configuration"))?;
            if ion.grant.as_ref().map(|g| &g.token) == token.as_ref() {
                ion.grant = Some(Grant::parse(&fetch_authorized(
                    &self.0,
                    &ion.endpoint,
                    Some(&ion.token),
                )?)?);
            }
            let g = ion
                .grant
                .as_ref()
                .ok_or(Error::Message("Missing ion grant"))?;
            let mut credits = self
                .2
                .lock()
                .map_err(|_| Error::Message("Ion credits unavailable"))?;
            for credit in &g.credits {
                if !credits.contains(credit) {
                    credits.push(credit.clone());
                }
            }
            if !g.authorizes(url) {
                return Err(Error::Message("Ion asset location changed; reopen source"));
            }
            g.token.clone()
        };
        self.request(url, Some(&new_token), false, HttpSource::Source)
    }

    pub(crate) fn load_root(&self, url: Option<Url>, sender: SyncSender<Completion>) {
        let loader = self.clone();
        std::thread::spawn(move || {
            let result = loader
                .root_url(url)
                .and_then(|url| loader.prepared_root(url))
                .and_then(|url| {
                    loader.fetch(&url).and_then(|b| {
                        loader
                            .2
                            .lock()
                            .map_err(|_| Error::Message("Credits unavailable"))?
                            .extend(crate::root_credits(&b, &url));
                        let nodes = parse_root(&b, &url)?;
                        if loader.3.is_some() && nodes.iter().all(|n| n.terrain.is_none()) {
                            return Err(Error::Message("Imagery overlay requires a terrain layer"));
                        }
                        Ok(nodes)
                    })
                });
            let _ = sender.send(Completion::Root(result));
        });
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn load_tile(
        &self,
        id: usize,
        url: Url,
        transform: DMat4,
        add: bool,
        depth: usize,
        terrain: Option<crate::terrain::TileAddress>,
        sender: SyncSender<Completion>,
    ) {
        let loader = self.clone();
        std::thread::spawn(move || {
            let bytes = loader.fetch(&url);
            let size = bytes.as_ref().map_or(0, Vec::len);
            let result = bytes.and_then(|b| {
                if let Some(address) = terrain {
                    let mut primitive = crate::terrain::decode(&b, address)?;
                    if let Some(imagery) = loader.3.as_ref() {
                        crate::terrain::apply_image(
                            &mut primitive,
                            &loader.fetch(&imagery.url(address)?)?,
                        )?;
                    }
                    return Ok(Payload::Mesh(
                        Tile {
                            id: id as u64,
                            primitives: vec![primitive],
                        },
                        vec![],
                    ));
                }
                if let Some(nodes) = content::external_tree(&b, transform, add, depth, &url)? {
                    return Ok(Payload::Hierarchy(nodes));
                }
                let credits = content::credits(&b)?;
                let primitives = content::decode_with(&b, transform, &url, &|u| loader.fetch(u))?;
                Ok(Payload::Mesh(
                    Tile {
                        id: id as u64,
                        primitives,
                    },
                    credits,
                ))
            });
            let _ = sender.send(Completion::Content(id, result, size));
        });
    }
}
pub(crate) fn fetch(agent: &ureq::Agent, url: &Url) -> Result<Vec<u8>> {
    fetch_authorized(agent, url, None)
}
fn fetch_authorized(agent: &ureq::Agent, url: &Url, token: Option<&str>) -> Result<Vec<u8>> {
    fetch_restricted(agent, url, token, false)
}
fn fetch_restricted(
    agent: &ureq::Agent,
    url: &Url,
    token: Option<&str>,
    strict: bool,
) -> Result<Vec<u8>> {
    use std::io::Read;
    const MAX: u64 = 32 * 1024 * 1024;
    let mut bytes = Vec::new();
    if url.scheme() == "file" {
        let p = url
            .to_file_path()
            .map_err(|_| Error::Message("Invalid file URL"))?;
        std::fs::File::open(p)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error::Message("File not found")
                } else {
                    Error::Message("File access failed")
                }
            })?
            .take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Message("File read failed"))?;
    } else {
        let mut request = agent.get(url.as_str()).header(
            "Accept",
            "application/vnd.quantized-mesh,application/octet-stream;q=0.9,application/json;q=0.8",
        );
        if strict || token.is_some() || crate::google_url(url) {
            request = request
                .config()
                .max_redirects(0)
                .max_redirects_will_error(true)
                .build();
        }
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let mut response = request
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|e| {
                Error::http(match e {
                    ureq::Error::Timeout(_) => HttpError::Timeout,
                    ureq::Error::TooManyRedirects => HttpError::Redirect,
                    _ => HttpError::Network,
                })
            })?;
        if !response.status().is_success() {
            return Err(Error::Http {
                kind: HttpError::Status(response.status().as_u16()),
                source: HttpSource::Source,
                retry_after: crate::error::retry_after(
                    response
                        .headers()
                        .get("Retry-After")
                        .and_then(|v| v.to_str().ok()),
                    std::time::SystemTime::now(),
                ),
            });
        }
        response
            .body_mut()
            .as_reader()
            .take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::http(HttpError::Body))?;
    }
    if bytes.len() as u64 > MAX {
        return Err(Error::Message("Response exceeds 32 MiB limit"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    #[test]
    fn shared_provider_throttle_stops_other_tiles_but_allows_package_reads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/tile", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            for response in [
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 60\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                assert!(stream.read(&mut [0; 4096]).unwrap() > 0);
                stream.write_all(response.as_bytes()).unwrap();
            }
            listener.set_nonblocking(true).unwrap();
            assert!(listener.accept().is_err());
        });
        let loader = Loader::default();
        assert_eq!(
            loader
                .request(&url, None, true, HttpSource::Provider)
                .unwrap_err()
                .status(),
            Some(429)
        );
        // No network request may escape via another worker or another tile ID.
        for id in 0..20 {
            assert_eq!(
                loader
                    .clone()
                    .request(
                        &url.join(&format!("tile{id}")).unwrap(),
                        None,
                        true,
                        HttpSource::Provider
                    )
                    .unwrap_err()
                    .status(),
                Some(429)
            );
        }
        assert_eq!(
            loader
                .request(&url, None, true, HttpSource::Package)
                .unwrap(),
            b"ok"
        );
        server.join().unwrap();
    }

    #[test]
    fn http_failures_preserve_status_source_and_retry_after_without_secrets() {
        for status in [401, 403, 404, 429, 500, 503] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let root = Url::parse(&format!(
                "http://{}/package/tileset.json",
                listener.local_addr().unwrap()
            ))
            .unwrap();
            let loader = Loader::prepared(crate::package::Package::new(&root, None).unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                assert!(stream.read(&mut [0; 4096]).unwrap() > 0);
                write!(stream, "HTTP/1.1 {status} Error\r\nRetry-After: 42\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret").unwrap();
            });
            let error = loader.fetch(&root).unwrap_err();
            assert_eq!(
                error,
                Error::Http {
                    kind: HttpError::Status(status),
                    source: HttpSource::Package,
                    retry_after: Some(42.)
                }
            );
            assert_eq!(
                error.to_string(),
                format!("HTTP {status} (prepared package); retry after 42.0s")
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn offline_assets_reject_redirects_and_external_references() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let root = Url::parse(&format!(
            "http://{}/package/tileset.json",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let loader = Loader::prepared(crate::package::Package::new(&root, None).unwrap());
        listener.set_nonblocking(true).unwrap();
        assert!(
            loader
                .fetch(&root.join("../outside.json").unwrap())
                .is_err()
        );
        assert!(
            listener.accept().is_err(),
            "offline package escaped its asset directory"
        );
        listener.set_nonblocking(false).unwrap();
        let location = root.join("../outside.json").unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(stream);
            listener.set_nonblocking(true).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            assert!(
                listener.accept().is_err(),
                "offline asset redirect was followed"
            );
        });
        assert!(loader.fetch(&root).is_err());
        server.join().unwrap();
    }

    #[test]
    fn only_missing_hybrid_files_fall_back_to_the_provider() {
        for (status, expected) in [
            (404, "Invalid ion asset or token"),
            (403, "HTTP access denied (403)"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let root = Url::parse(&format!(
                "http://{}/package/tileset.json",
                listener.local_addr().unwrap()
            ))
            .unwrap();
            let remote = Url::parse("https://tile.googleapis.com/v1/3dtiles/root.json").unwrap();
            let mut package = crate::package::Package::new(&root, Some("unused")).unwrap();
            package.install(&serde_json::to_vec(&serde_json::json!({"version":1,"ion_asset_id":2275207,"root":remote.as_str(),"resources":{remote.as_str():"source/root.json"}})).unwrap()).unwrap();
            // Fail at provider credential validation, before any external request.
            // This distinguishes fallback from an error returned by the asset host.
            package.token = Some("\n".into());
            let loader = Loader::prepared(package);
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(
                    stream,
                    "HTTP/1.1 {status} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            });
            assert_eq!(loader.fetch(&remote).unwrap_err().message(), expected);
            server.join().unwrap();
        }
    }

    #[test]
    fn slow_provider_does_not_block_prepared_reads() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let directory =
            std::env::temp_dir().join(format!("prepared-progress-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("cached.json"), b"cached terrain").unwrap();
        let root = Url::from_file_path(directory.join("tileset.json")).unwrap();
        let remote = Url::parse("https://tile.googleapis.com/v1/3dtiles/root.json").unwrap();
        let mut package = crate::package::Package::new(&root, Some("test-token")).unwrap();
        package
            .install(
                &serde_json::to_vec(&serde_json::json!({
                    "version": 1, "ion_asset_id": 2275207, "root": remote.as_str(),
                    "resources": {remote.as_str(): "cached.json"}
                }))
                .unwrap(),
            )
            .unwrap();
        let mut loader = Loader::prepared(package);
        loader.0 = ureq::Agent::config_builder()
            .proxy(Some(
                ureq::Proxy::new(&format!("http://{}", proxy.local_addr().unwrap())).unwrap(),
            ))
            .timeout_global(Some(Duration::from_secs(3)))
            .build()
            .into();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = proxy.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            // No provider connection is ever made: this local proxy rejects it.
            let _ = stream.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n");
        });
        let provider = loader.clone();
        let provider_thread = std::thread::spawn(move || {
            provider.provider_url(
                &Url::parse("https://tile.googleapis.com/v1/3dtiles/missing.json").unwrap(),
                false,
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let (cached_tx, cached_rx) = std::sync::mpsc::channel();
        let cached_thread = std::thread::spawn(move || {
            cached_tx.send(loader.fetch(&remote)).unwrap();
        });
        let cached = cached_rx.recv_timeout(Duration::from_millis(500));
        release_tx.send(()).unwrap();
        server.join().unwrap();
        assert!(provider_thread.join().unwrap().is_err());
        cached_thread.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(
            cached
                .expect("cached terrain waited for the provider")
                .unwrap(),
            b"cached terrain"
        );
    }

    #[test]
    fn hybrid_reads_cached_resources_without_contacting_ion() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let root = Url::parse(&format!(
            "http://{}/package/tileset.json",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let loader = Loader::prepared(
            crate::package::Package::new(&root, Some("invalid-token-must-never-be-used")).unwrap(),
        );
        let remote = "https://tile.googleapis.com/v1/3dtiles/root.json";
        let server = std::thread::spawn(move || {
            for (path, body) in [
                ("/package/package.json", serde_json::json!({"version":1,"ion_asset_id":2275207,"root":remote,"resources":{(remote):"source/root.json"}}).to_string()),
                ("/package/source/root.json", "cached root".to_owned()),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = [0; 4096];
                let count = stream.read(&mut bytes).unwrap();
                let request = std::str::from_utf8(&bytes[..count]).unwrap();
                assert!(request.starts_with(&format!("GET {path} ")));
                assert!(!request.contains("invalid-token"));
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let source = loader.prepared_root(root).unwrap();
        assert_eq!(loader.fetch(&source).unwrap(), b"cached root");
        assert!(
            loader
                .4
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .credentials
                .is_none()
        );
        server.join().unwrap();
    }

    #[test]
    fn authenticated_redirect_is_not_followed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!(
            "http://{}/redirect",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let location = url.join("other").unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buf = [0; 2048];
            let mut request = Vec::new();
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
            }
            assert!(String::from_utf8_lossy(&request).contains("Bearer secret"));
            write!(stream, "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            drop(stream);
            listener.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_millis(200);
            while Instant::now() < deadline {
                assert!(
                    listener.accept().is_err(),
                    "authenticated redirect was followed"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        assert!(fetch_authorized(&Loader::default().0, &url, Some("secret")).is_err());
        server.join().unwrap();
    }

    #[test]
    fn ion_resolves_refreshes_and_keeps_master_token_off_asset_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server_base = base.clone();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            for step in 0..5 {
                let mut stream = loop {
                    if let Ok((s, _)) = listener.accept() {
                        break s;
                    }
                    assert!(Instant::now() < deadline, "missing expected request {step}");
                    std::thread::sleep(Duration::from_millis(5));
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buf[..n]);
                }
                let request = String::from_utf8(request).unwrap().to_lowercase();
                let (status, body) = match step {
                    0 | 3 => {
                        assert!(request.starts_with("get /endpoint "));
                        assert!(request.contains("authorization: bearer master"));
                        let token = if step == 0 { "old" } else { "new" };
                        (
                            200,
                            format!(
                                r#"{{"type":"3DTILES","url":"{server_base}/42/tileset.json","accessToken":"{token}","attributions":[{{"html":"provider"}}]}}"#
                            ),
                        )
                    }
                    1 => {
                        assert!(request.starts_with("get /42/tileset.json "));
                        assert!(request.contains("authorization: bearer old"));
                        assert!(!request.contains("master"));
                        (
                            200,
                            include_str!("../../../../tests/data/tileset.json").into(),
                        )
                    }
                    2 => {
                        assert!(request.starts_with("get /42/triangle.gltf "));
                        assert!(request.contains("authorization: bearer old"));
                        (401, String::new())
                    }
                    _ => {
                        assert!(request.starts_with("get /42/triangle.gltf "));
                        assert!(request.contains("authorization: bearer new"));
                        assert!(!request.contains("master"));
                        (
                            200,
                            include_str!("../../../../tests/data/triangle.gltf").into(),
                        )
                    }
                };
                write!(stream, "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let mut ion = Ion::new(42, "master").unwrap();
        ion.endpoint = Url::parse(&format!("{base}/endpoint")).unwrap();
        let loader = Loader::ion(ion);
        let mut session = crate::Session::start(loader, None).unwrap();
        let view = crate::View {
            position: [6_378_200.0, 5.0, -5.0],
            direction: [-1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            width: 1280.0,
            height: 720.0,
            horizontal_fov: 1.4,
            vertical_fov: 0.9,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let frame = session.update(&[view], 0.016).unwrap();
            assert!(!frame.load_failed, "{:?}", frame.error);
            if !frame.loaded.is_empty() {
                assert_eq!(frame.loaded.len(), 1);
                assert_eq!(frame.loaded[0].primitives.len(), 1);
                assert_eq!(frame.credits, ["provider"]);
                assert!(!frame.loaded[0].primitives[0].pixels.is_empty());
                break;
            }
            assert!(Instant::now() < deadline, "ion fixture timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        server.join().unwrap();
    }
}
