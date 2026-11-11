use crate::ion::{Grant, Ion};
use crate::{Completion, Error, Payload, Result, Tile, content, parse_root};
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
        )
    }
}
impl Loader {
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
            .map_err(|_| Error("Package unavailable"))?
            .as_ref()
            .filter(|p| p.token.is_some())
            .map(|p| p.local("package.json"))
            .transpose()?;
        let Some(manifest_url) = manifest_url else {
            return Ok(url);
        };
        let bytes = fetch_restricted(&self.0, &manifest_url, None, true)?;
        self.4
            .lock()
            .map_err(|_| Error("Package unavailable"))?
            .as_mut()
            .ok_or(Error("Package unavailable"))?
            .install(&bytes)
    }
    fn provider_url(&self, url: &Url, refresh: bool) -> Result<Url> {
        // Serialize grant creation: concurrent cache misses share one session.
        let mut state = self.4.lock().map_err(|_| Error("Package unavailable"))?;
        let package = state.as_mut().ok_or(Error("Package unavailable"))?;
        if package.credentials.is_none() || refresh {
            let manifest = package
                .manifest
                .as_ref()
                .ok_or(Error("Missing package manifest"))?;
            let ion = Ion::new(
                manifest.ion_asset_id,
                package.token.as_deref().ok_or(Error("Offline terrain"))?,
            )?;
            let grant = Grant::parse(&fetch_authorized(&self.0, &ion.endpoint, Some(&ion.token))?)?;
            crate::package::canonical(&grant.url)?;
            let bytes = fetch_restricted(&self.0, &grant.url, None, true)?;
            package.credentials = Some(crate::package::GoogleSession::from_root(
                &grant.url, &bytes,
            )?);
            self.2
                .lock()
                .map_err(|_| Error("Credits unavailable"))?
                .extend(grant.credits);
        }
        package
            .credentials
            .as_ref()
            .ok_or(Error("Missing Photo 3D session"))?
            .authorize(url)
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
        let mut state = self.1.lock().map_err(|_| Error("Ion state unavailable"))?;
        let ion = state.as_mut().ok_or(Error("Missing ion configuration"))?;
        ion.grant = Some(Grant::parse(&fetch_authorized(
            &self.0,
            &ion.endpoint,
            Some(&ion.token),
        )?)?);
        let grant = ion.grant.as_ref().unwrap();
        *self
            .2
            .lock()
            .map_err(|_| Error("Ion credits unavailable"))? = grant.credits.clone();
        Ok(grant.url.clone())
    }
    fn fetch(&self, url: &Url) -> Result<Vec<u8>> {
        let request = self
            .4
            .lock()
            .map_err(|_| Error("Package unavailable"))?
            .as_ref()
            .map(|p| p.request(url))
            .transpose()?;
        if let Some(request) = request {
            let remote = match request {
                crate::package::Request::Local(local) => {
                    return fetch_restricted(&self.0, &local, None, true);
                }
                crate::package::Request::Cached { local, remote } => {
                    match fetch_restricted(&self.0, &local, None, true) {
                        Ok(bytes) => return Ok(bytes),
                        Err(Error("HTTP not found" | "File not found")) => remote,
                        Err(error) => return Err(error),
                    }
                }
                crate::package::Request::Provider(remote) => remote,
            };
            let authorized = self.provider_url(&remote, false)?;
            let result = fetch_restricted(&self.0, &authorized, None, true);
            if matches!(
                result,
                Err(Error("HTTP unauthorized" | "HTTP access denied"))
            ) {
                let refreshed = self.provider_url(&remote, true)?;
                return fetch_restricted(&self.0, &refreshed, None, true);
            } else {
                return result;
            }
        }

        let token = {
            let state = self.1.lock().map_err(|_| Error("Ion state unavailable"))?;
            state
                .as_ref()
                .and_then(|i| i.grant.as_ref())
                .filter(|g| g.authorizes(url))
                .map(|g| g.token.clone())
        };
        let result = fetch_authorized(&self.0, url, token.as_deref());
        if !matches!(result, Err(Error("HTTP unauthorized"))) || token.is_none() {
            return result;
        }
        let new_token = {
            let mut state = self.1.lock().map_err(|_| Error("Ion state unavailable"))?;
            let ion = state.as_mut().ok_or(Error("Missing ion configuration"))?;
            if ion.grant.as_ref().map(|g| &g.token) == token.as_ref() {
                ion.grant = Some(Grant::parse(&fetch_authorized(
                    &self.0,
                    &ion.endpoint,
                    Some(&ion.token),
                )?)?);
            }
            let g = ion.grant.as_ref().ok_or(Error("Missing ion grant"))?;
            let mut credits = self
                .2
                .lock()
                .map_err(|_| Error("Ion credits unavailable"))?;
            for credit in &g.credits {
                if !credits.contains(credit) {
                    credits.push(credit.clone());
                }
            }
            if !g.authorizes(url) {
                return Err(Error("Ion asset location changed; reopen source"));
            }
            g.token.clone()
        };
        fetch_authorized(&self.0, url, Some(&new_token))
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
                            .map_err(|_| Error("Credits unavailable"))?
                            .extend(crate::root_credits(&b, &url));
                        let nodes = parse_root(&b, &url)?;
                        if loader.3.is_some() && nodes.iter().all(|n| n.terrain.is_none()) {
                            return Err(Error("Imagery overlay requires a terrain layer"));
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
        let p = url.to_file_path().map_err(|_| Error("Invalid file URL"))?;
        std::fs::File::open(p)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Error("File not found")
                } else {
                    Error("File access failed")
                }
            })?
            .take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("File read failed"))?;
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
        let mut response = request.call().map_err(|e| match e {
            ureq::Error::StatusCode(401) => Error("HTTP unauthorized"),
            ureq::Error::StatusCode(403) => Error("HTTP access denied"),
            ureq::Error::StatusCode(404) => Error("HTTP not found"),
            _ => Error("HTTP request failed"),
        })?;
        if !response.status().is_success() {
            return Err(Error("HTTP redirect or unexpected status"));
        }
        response
            .body_mut()
            .as_reader()
            .take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("HTTP body failed"))?;
    }
    if bytes.len() as u64 > MAX {
        return Err(Error("Response exceeds 32 MiB limit"));
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
            (403, "HTTP access denied"),
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
            assert_eq!(loader.fetch(&remote).unwrap_err().0, expected);
            server.join().unwrap();
        }
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
