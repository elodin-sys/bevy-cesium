use crate::ion::{Grant, Ion};
use crate::{Completion, Error, Payload, Result, Tile, content, parse_root};
use crate::{HttpError, HttpSource};
use glam::DMat4;
use js_sys::{Reflect, Uint8Array};
use std::{cell::RefCell, rc::Rc};
use std::{collections::HashMap, sync::mpsc::Sender};
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
    Rc<RefCell<crate::retry::Cooldowns>>,
    Rc<RefCell<Option<(js_sys::Promise, js_sys::Function)>>>,
);

/// Held while one job signs in to the provider. Dropping it wakes the waiters.
struct ProviderGate(Rc<RefCell<Option<(js_sys::Promise, js_sys::Function)>>>);

impl Drop for ProviderGate {
    fn drop(&mut self) {
        if let Some((_, release)) = self.0.borrow_mut().take() {
            let _ = release.call0(&JsValue::UNDEFINED);
        }
    }
}

impl Loader {
    async fn request(
        &self,
        url: &Url,
        token: Option<&str>,
        strict: bool,
        source: HttpSource,
    ) -> Result<Vec<u8>> {
        self.4.borrow().check(source, js_sys::Date::now() / 1000.)?;
        let result = fetch_restricted(url, token, strict, source == HttpSource::Package)
            .await
            .map_err(|e| e.with_source(source));
        if let Err(error) = result {
            self.4
                .borrow_mut()
                .observe(error, js_sys::Date::now() / 1000.);
        }
        result
    }

    pub(crate) fn requires_provider(&self, url: &Url) -> bool {
        self.3.borrow().as_ref().is_some_and(|package| {
            matches!(
                package.request(url),
                Ok(crate::package::Request::Provider(_))
            )
        })
    }

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
        let bytes = self
            .request(&manifest_url, None, true, HttpSource::Package)
            .await?;
        self.3
            .borrow_mut()
            .as_mut()
            .ok_or(Error::Message("Package unavailable"))?
            .install(&bytes)
    }
    async fn provider_url(&self, url: &Url, refresh: bool) -> Result<Url> {
        // Share one grant between provider misses, like the native loader's
        // lock: a job that waited reuses the session the first one stored.
        loop {
            let held = self.5.borrow().as_ref().map(|(held, _)| held.clone());
            let Some(held) = held else { break };
            let _ = JsFuture::from(held).await;
        }
        let mut release = None;
        let held = js_sys::Promise::new(&mut |resolve, _| release = Some(resolve));
        if let Some(release) = release {
            *self.5.borrow_mut() = Some((held, release));
        }
        let _gate = ProviderGate(self.5.clone());
        self.sign_in(url, refresh).await
    }

    async fn sign_in(&self, url: &Url, refresh: bool) -> Result<Url> {
        let ion = {
            let state = self.3.borrow();
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
        let grant = Grant::parse(
            &self
                .request(&ion.endpoint, Some(&ion.token), false, HttpSource::Provider)
                .await?,
        )?;
        crate::package::canonical(&grant.url)?;
        let bytes = self
            .request(&grant.url, None, true, HttpSource::Provider)
            .await?;
        let credentials = crate::package::GoogleSession::from_root(&grant.url, &bytes)?;
        let authorized = credentials.authorize(url)?;
        self.3
            .borrow_mut()
            .as_mut()
            .ok_or(Error::Message("Package unavailable"))?
            .credentials = Some(credentials);
        self.1.borrow_mut().extend(grant.credits);
        Ok(authorized)
    }

    pub(crate) fn with_imagery(mut self, imagery: Option<crate::terrain::Imagery>) -> Self {
        self.2 = Rc::new(imagery);
        self
    }
    pub(crate) fn ion(ion: Ion) -> Self {
        Self {
            0: Rc::new(RefCell::new(Some(ion))),
            ..Self::default()
        }
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
            let ion = state
                .as_ref()
                .ok_or(Error::Message("Missing ion configuration"))?;
            (ion.endpoint.clone(), ion.token.clone())
        };
        let grant = Grant::parse(&fetch_authorized(&endpoint, Some(&token)).await?)?;
        let url = grant.url.clone();
        self.0
            .borrow_mut()
            .as_mut()
            .ok_or(Error::Message("Missing ion configuration"))?
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
                    return self.request(&local, None, true, HttpSource::Package).await;
                }
                crate::package::Request::Cached { local, remote } => {
                    match self.request(&local, None, true, HttpSource::Package).await {
                        Ok(bytes) => return Ok(bytes),
                        Err(e) if e.status() == Some(404) => remote,
                        Err(error) => return Err(error),
                    }
                }
                crate::package::Request::Provider(remote) => remote,
            };
            let authorized = self
                .provider_url(&remote, false)
                .await
                .map_err(|e| e.with_source(HttpSource::Provider))?;
            let result = self
                .request(&authorized, None, true, HttpSource::Provider)
                .await;
            if matches!(
                result.as_ref().err().and_then(|e| e.status()),
                Some(401 | 403)
            ) {
                let refreshed = self
                    .provider_url(&remote, true)
                    .await
                    .map_err(|e| e.with_source(HttpSource::Provider))?;
                return self
                    .request(&refreshed, None, true, HttpSource::Provider)
                    .await;
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
        let result = self
            .request(url, token.as_deref(), false, HttpSource::Source)
            .await;
        if result.as_ref().err().and_then(|e| e.status()) != Some(401) || token.is_none() {
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
            .ok_or(Error::Message("Ion asset location changed; reopen source"))?;
        self.request(url, Some(&token), false, HttpSource::Source)
            .await
    }

    pub(crate) fn load_root(&self, url: Option<Url>, sender: Sender<Completion>) {
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
                        return Err(Error::Message("Imagery overlay requires a terrain layer"));
                    }
                    Ok(nodes)
                })
            }
            .await;
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
        sender: Sender<Completion>,
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
            let _ = sender.send(Completion::Content(id, result, size));
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
            return Err(Error::Message("External resource budget exceeded"));
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
            .ok_or(Error::Message("Resource was not prefetched"))
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
    fetch_restricted(url, token, false, false).await
}

/// A host page may deliver prepared package files itself, for example in
/// batches or from its own cache. Provider requests never reach it.
fn package_hook() -> Option<js_sys::Function> {
    Reflect::get(&js_sys::global(), &JsValue::from_str("elodinFetchPackage"))
        .ok()?
        .dyn_into()
        .ok()
}

async fn fetch_restricted(
    url: &Url,
    token: Option<&str>,
    strict: bool,
    package: bool,
) -> Result<Vec<u8>> {
    const MAX: usize = 32 * 1024 * 1024;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(Error::Message("Browser sources must use HTTP or HTTPS"));
    }
    let controller =
        AbortController::new().map_err(|_| Error::Message("AbortController unavailable"))?;
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
        .map_err(|_| Error::Message("Invalid browser request"))?;
    request
        .headers()
        .set(
            "Accept",
            "application/vnd.quantized-mesh,application/octet-stream;q=0.9,application/json;q=0.8",
        )
        .map_err(|_| Error::Message("Invalid request header"))?;
    if let Some(token) = token {
        request
            .headers()
            .set("Authorization", &format!("Bearer {token}"))
            .map_err(|_| Error::Message("Invalid access token"))?;
    }
    let window = web_sys::window().ok_or(Error::Message("Browser window unavailable"))?;
    let fetched = match package.then(package_hook).flatten() {
        // A hook that throws fails like a network error, so the tile is retried.
        Some(hook) => hook
            .call1(&js_sys::global(), &request)
            .map(|response| js_sys::Promise::resolve(&response))
            .unwrap_or_else(|error| js_sys::Promise::reject(&error)),
        None => window.fetch_with_request(&request),
    };
    let response: Response = JsFuture::from(fetched)
        .await
        .map_err(|_| {
            Error::http(if guard.controller.signal().aborted() {
                HttpError::Timeout
            } else {
                HttpError::BrowserNetwork
            })
        })?
        .dyn_into()
        .map_err(|_| Error::Message("Invalid HTTP response"))?;
    if !response.ok() {
        return Err(Error::Http {
            kind: HttpError::Status(response.status()),
            source: HttpSource::Source,
            retry_after: crate::error::retry_after(
                response
                    .headers()
                    .get("Retry-After")
                    .ok()
                    .flatten()
                    .as_deref(),
                std::time::UNIX_EPOCH
                    + std::time::Duration::from_secs_f64(js_sys::Date::now() / 1000.),
            ),
        });
    }
    if response
        .headers()
        .get("Content-Length")
        .ok()
        .flatten()
        .and_then(|n| n.parse::<usize>().ok())
        .is_some_and(|n| n > MAX)
    {
        return Err(Error::Message("Response exceeds 32 MiB limit"));
    }
    let reader: ReadableStreamDefaultReader = response
        .body()
        .ok_or(Error::Message("Missing HTTP body"))?
        .get_reader()
        .dyn_into()
        .map_err(|_| Error::Message("HTTP stream unavailable"))?;
    let mut bytes = Vec::new();
    loop {
        let chunk = JsFuture::from(reader.read())
            .await
            .map_err(|_| Error::http(HttpError::Body))?;
        if Reflect::get(&chunk, &JsValue::from_str("done"))
            .ok()
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            break;
        }
        let value = Reflect::get(&chunk, &JsValue::from_str("value"))
            .map_err(|_| Error::Message("Invalid HTTP body chunk"))?;
        let data = Uint8Array::new(&value);
        if bytes.len() + data.length() as usize > MAX {
            return Err(Error::Message("Response exceeds 32 MiB limit"));
        }
        let offset = bytes.len();
        bytes.resize(offset + data.length() as usize, 0);
        data.copy_to(&mut bytes[offset..]);
    }
    Ok(bytes)
}
