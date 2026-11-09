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
        )
    }
}
impl Loader {
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
            let result = loader.root_url(url).and_then(|url| {
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
    use std::io::Read;
    const MAX: u64 = 32 * 1024 * 1024;
    let mut bytes = Vec::new();
    if url.scheme() == "file" {
        let p = url.to_file_path().map_err(|_| Error("Invalid file URL"))?;
        std::fs::File::open(p)
            .map_err(|_| Error("File not found"))?
            .take(MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error("File read failed"))?;
    } else {
        let mut request = agent.get(url.as_str()).header(
            "Accept",
            "application/vnd.quantized-mesh,application/octet-stream;q=0.9,application/json;q=0.8",
        );
        if token.is_some() || crate::google_url(url) {
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
