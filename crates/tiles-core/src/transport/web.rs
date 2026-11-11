use crate::ion::{Grant, Ion};
use crate::{Completion, Error, Payload, Result, Tile, content, parse_root};
use glam::DMat4;
use js_sys::{Reflect, Uint8Array};
use std::{cell::RefCell, rc::Rc};
use std::{collections::HashMap, sync::mpsc::SyncSender};
use url::Url;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    AbortController, ReadableStreamDefaultReader, Request, RequestInit, RequestMode, Response,
};

#[derive(Clone, Default)]
pub(crate) struct Loader(
    Rc<RefCell<Option<Ion>>>,
    Rc<RefCell<Vec<String>>>,
    Rc<Option<crate::terrain::Imagery>>,
    Rc<RefCell<Option<crate::package::Package>>>,
);

impl Loader {
    pub(crate) fn prepared(package: crate::package::Package) -> Self {
        Self {
            3: Rc::new(RefCell::new(Some(package))),
            ..Self::default()
        }
    }
    async fn prepared_root(&self, url: Url) -> Result<Url> {
        let manifest_url = self
            .3
            .borrow()
            .as_ref()
            .filter(|p| p.token.is_some())
            .map(|p| p.local("package.json"))
            .transpose()?;
        let Some(manifest_url) = manifest_url else {
            return Ok(url);
        };
        let bytes = fetch_restricted(&manifest_url, None, true).await?;
        self.3
            .borrow_mut()
            .as_mut()
            .ok_or(Error("Package unavailable"))?
            .install(&bytes)
    }
    async fn provider_url(&self, url: &Url, refresh: bool) -> Result<Url> {
        let ion = {
            let state = self.3.borrow();
            let package = state.as_ref().ok_or(Error("Package unavailable"))?;
            if !refresh && let Some(credentials) = &package.credentials {
                return credentials.authorize(url);
            }
            let manifest = package
                .manifest
                .as_ref()
                .ok_or(Error("Missing package manifest"))?;
            Ion::new(
                manifest.ion_asset_id,
                package.token.as_deref().ok_or(Error("Offline terrain"))?,
            )?
        };
        let grant = Grant::parse(&fetch_authorized(&ion.endpoint, Some(&ion.token)).await?)?;
        crate::package::canonical(&grant.url)?;
        let bytes = fetch_restricted(&grant.url, None, true).await?;
        let credentials = crate::package::GoogleSession::from_root(&grant.url, &bytes)?;
        let authorized = credentials.authorize(url)?;
        self.3
            .borrow_mut()
            .as_mut()
            .ok_or(Error("Package unavailable"))?
            .credentials = Some(credentials);
        self.1.borrow_mut().extend(grant.credits);
        Ok(authorized)
    }

    pub(crate) fn with_imagery(mut self, imagery: Option<crate::terrain::Imagery>) -> Self {
        self.2 = Rc::new(imagery);
        self
    }
    pub(crate) fn ion(ion: Ion) -> Self {
        Self(
            Rc::new(RefCell::new(Some(ion))),
            Rc::default(),
            Rc::default(),
            Rc::default(),
        )
    }
    pub(crate) fn credits(&self) -> Vec<String> {
        let mut credits = self
            .0
            .borrow()
            .as_ref()
            .and_then(|i| i.grant.as_ref().map(|g| g.credits.clone()))
            .unwrap_or_default();
        credits.extend(self.1.borrow().clone());
        if let Some(imagery) = self.2.as_ref() {
            credits.push(imagery.credit.clone());
        }
        credits
    }
    async fn refresh(&self) -> Result<Url> {
        let (endpoint, token) = {
            let state = self.0.borrow();
            let ion = state.as_ref().ok_or(Error("Missing ion configuration"))?;
            (ion.endpoint.clone(), ion.token.clone())
        };
        let grant = Grant::parse(&fetch_authorized(&endpoint, Some(&token)).await?)?;
        let url = grant.url.clone();
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or(Error("Missing ion configuration"))?
            .grant = Some(grant);
        Ok(url)
    }
    async fn fetch(&self, url: &Url) -> Result<Vec<u8>> {
        let request = self
            .3
            .borrow()
            .as_ref()
            .map(|p| p.request(url))
            .transpose()?;
        if let Some(request) = request {
            let remote = match request {
                crate::package::Request::Local(local) => {
                    return fetch_restricted(&local, None, true).await;
                }
                crate::package::Request::Cached { local, remote } => {
                    match fetch_restricted(&local, None, true).await {
                        Ok(bytes) => return Ok(bytes),
                        Err(Error("HTTP not found")) => remote,
                        Err(error) => return Err(error),
                    }
                }
                crate::package::Request::Provider(remote) => remote,
            };
            let authorized = self.provider_url(&remote, false).await?;
            let result = fetch_restricted(&authorized, None, true).await;
            if matches!(
                result,
                Err(Error("HTTP unauthorized" | "HTTP access denied"))
            ) {
                let refreshed = self.provider_url(&remote, true).await?;
                return fetch_restricted(&refreshed, None, true).await;
            } else {
                return result;
            }
        }

        let token = self
            .0
            .borrow()
            .as_ref()
            .and_then(|i| i.grant.as_ref())
            .filter(|g| g.authorizes(url))
            .map(|g| g.token.clone());
        let result = fetch_authorized(url, token.as_deref()).await;
        if !matches!(result, Err(Error("HTTP unauthorized"))) || token.is_none() {
            return result;
        }
        let unchanged = self
            .0
            .borrow()
            .as_ref()
            .and_then(|i| i.grant.as_ref())
            .map(|g| &g.token)
            == token.as_ref();
        if unchanged {
            self.refresh().await?;
        }
        let token = self
            .0
            .borrow()
            .as_ref()
            .and_then(|i| i.grant.as_ref())
            .filter(|g| g.authorizes(url))
            .map(|g| g.token.clone())
            .ok_or(Error("Ion asset location changed; reopen source"))?;
        fetch_authorized(url, Some(&token)).await
    }

    pub(crate) fn load_root(&self, url: Option<Url>, sender: SyncSender<Completion>) {
        let loader = self.clone();
        spawn_local(async move {
            let result = async {
                let url = match url {
                    Some(url) => url,
                    None => loader.refresh().await?,
                };
                let url = loader.prepared_root(url).await?;
                loader.fetch(&url).await.and_then(|b| {
                    loader.1.borrow_mut().extend(crate::root_credits(&b, &url));
                    let nodes = parse_root(&b, &url)?;
                    if loader.2.is_some() && nodes.iter().all(|n| n.terrain.is_none()) {
                        return Err(Error("Imagery overlay requires a terrain layer"));
                    }
                    Ok(nodes)
                })
            }
            .await;
            let _ = sender.try_send(Completion::Root(result));
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
        spawn_local(async move {
            let bytes = loader.fetch(&url).await;
            let size = bytes.as_ref().map_or(0, Vec::len);
            let result = match bytes {
                Ok(bytes) => {
                    async {
                        if let Some(address) = terrain {
                            let mut primitive = crate::terrain::decode(&bytes, address)?;
                            if let Some(imagery) = loader.2.as_ref() {
                                crate::terrain::apply_image(
                                    &mut primitive,
                                    &loader.fetch(&imagery.url(address)?).await?,
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
                        if let Some(nodes) =
                            content::external_tree(&bytes, transform, add, depth, &url)?
                        {
                            return Ok(Payload::Hierarchy(nodes));
                        }
                        let credits = content::credits(&bytes)?;
                        let primitives = decode_tile(&bytes, transform, &url, &loader).await?;
                        Ok(Payload::Mesh(
                            Tile {
                                id: id as u64,
                                primitives,
                            },
                            credits,
                        ))
                    }
                    .await
                }
                Err(error) => Err(error),
            };
            // Session permits at most four outstanding tile jobs; the channel
            // has four slots. No blocking synchronization on the browser thread.
            let _ = sender.try_send(Completion::Content(id, result, size));
        });
    }
}

async fn decode_tile(
    bytes: &[u8],
    transform: DMat4,
    url: &Url,
    loader: &Loader,
) -> Result<Vec<crate::Primitive>> {
    let mut resources = HashMap::new();
    let mut total = 0;
    for resource in content::external_resources(bytes, url)? {
        let data = loader.fetch(&resource).await?;
        total += data.len();
        if total > 64 * 1024 * 1024 {
            return Err(Error("External resource budget exceeded"));
        }
        resources.insert(resource, data);
    }
    // Let input/render tasks run before CPU decoding. Decode itself is currently
    // synchronous; a worker is a subsequent optimization, not hidden threading.
    gloo_timers::future::TimeoutFuture::new(0).await;
    content::decode_with(bytes, transform, url, &|resource| {
        resources
            .get(resource)
            .cloned()
            .ok_or(Error("Resource was not prefetched"))
    })
}

struct FetchGuard {
    controller: AbortController,
    _timeout: gloo_timers::callback::Timeout,
}
impl Drop for FetchGuard {
    fn drop(&mut self) {
        self.controller.abort();
    }
}

async fn fetch_authorized(url: &Url, token: Option<&str>) -> Result<Vec<u8>> {
    fetch_restricted(url, token, false).await
}
async fn fetch_restricted(url: &Url, token: Option<&str>, strict: bool) -> Result<Vec<u8>> {
    const MAX: usize = 32 * 1024 * 1024;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(Error("Browser sources must use HTTP or HTTPS"));
    }
    let controller = AbortController::new().map_err(|_| Error("AbortController unavailable"))?;
    let abort = controller.clone();
    let guard = FetchGuard {
        controller,
        _timeout: gloo_timers::callback::Timeout::new(20_000, move || abort.abort()),
    };
    let init = RequestInit::new();
    init.set_mode(RequestMode::Cors);
    if strict || token.is_some() || crate::google_url(url) {
        init.set_redirect(web_sys::RequestRedirect::Error);
    }
    init.set_signal(Some(&guard.controller.signal()));
    let request = Request::new_with_str_and_init(url.as_str(), &init)
        .map_err(|_| Error("Invalid browser request"))?;
    request
        .headers()
        .set(
            "Accept",
            "application/vnd.quantized-mesh,application/octet-stream;q=0.9,application/json;q=0.8",
        )
        .map_err(|_| Error("Invalid request header"))?;
    if let Some(token) = token {
        request
            .headers()
            .set("Authorization", &format!("Bearer {token}"))
            .map_err(|_| Error("Invalid access token"))?;
    }
    let window = web_sys::window().ok_or(Error("Browser window unavailable"))?;
    let response: Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| Error("HTTP request failed (network, CORS or timeout)"))?
        .dyn_into()
        .map_err(|_| Error("Invalid HTTP response"))?;
    if response.status() == 401 {
        return Err(Error("HTTP unauthorized"));
    }
    if response.status() == 403 {
        return Err(Error("HTTP access denied"));
    }
    if response.status() == 404 {
        return Err(Error("HTTP not found"));
    }
    if !response.ok() {
        return Err(Error("HTTP status error"));
    }
    if response
        .headers()
        .get("Content-Length")
        .ok()
        .flatten()
        .and_then(|n| n.parse::<usize>().ok())
        .is_some_and(|n| n > MAX)
    {
        return Err(Error("Response exceeds 32 MiB limit"));
    }
    let reader: ReadableStreamDefaultReader = response
        .body()
        .ok_or(Error("Missing HTTP body"))?
        .get_reader()
        .dyn_into()
        .map_err(|_| Error("HTTP stream unavailable"))?;
    let mut bytes = Vec::new();
    loop {
        let chunk = JsFuture::from(reader.read())
            .await
            .map_err(|_| Error("HTTP body failed"))?;
        if Reflect::get(&chunk, &JsValue::from_str("done"))
            .ok()
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            break;
        }
        let value = Reflect::get(&chunk, &JsValue::from_str("value"))
            .map_err(|_| Error("Invalid HTTP body chunk"))?;
        let data = Uint8Array::new(&value);
        if bytes.len() + data.length() as usize > MAX {
            return Err(Error("Response exceeds 32 MiB limit"));
        }
        let offset = bytes.len();
        bytes.resize(offset + data.length() as usize, 0);
        data.copy_to(&mut bytes[offset..]);
    }
    Ok(bytes)
}
