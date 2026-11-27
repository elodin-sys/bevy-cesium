#![forbid(unsafe_code)]
//! Experimental, bounded 3D Tiles streamer. No Cesium Native or C++ linkage.
mod content;
mod geography;
mod ion;
mod package;
mod retry;
pub mod terrain;
#[cfg(test)]
mod tests;
mod tree;
#[cfg(not(target_arch = "wasm32"))]
pub use content::decode;
#[cfg(not(target_arch = "wasm32"))]
#[path = "transport/native.rs"]
mod transport;
#[cfg(target_arch = "wasm32")]
#[path = "transport/web.rs"]
mod transport;
use glam::DVec3;
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use transport::fetch;
pub use tree::{Node, parse_tileset};
use url::Url;

mod error;
pub use error::{Error, HttpError, HttpSource};
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub position: [f64; 3],
    pub direction: [f64; 3],
    pub up: [f64; 3],
    pub width: f64,
    pub height: f64,
    pub horizontal_fov: f64,
    pub vertical_fov: f64,
}
impl View {
    fn valid(&self) -> bool {
        let d = DVec3::from_array(self.direction);
        let u = DVec3::from_array(self.up);
        DVec3::from_array(self.position).is_finite()
            && d.is_finite()
            && u.is_finite()
            && d.cross(u).length_squared() > 1e-12
            && d.length_squared().is_finite()
            && u.length_squared().is_finite()
            && [self.width, self.height]
                .iter()
                .all(|x| x.is_finite() && *x > 0.)
            && [self.horizontal_fov, self.vertical_fov]
                .iter()
                .all(|x| x.is_finite() && *x > 0. && *x < std::f64::consts::PI)
    }
}
#[derive(Debug)]
pub struct Primitive {
    pub transform: [f64; 16],
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub uvs: Vec<f32>,
    pub indices: Vec<u32>,
    pub base_color: [f32; 4],
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub double_sided: bool,
    pub unlit: bool,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub alpha_mode: u8,
    pub alpha_cutoff: f32,
}
#[derive(Debug)]
pub struct Tile {
    pub id: u64,
    pub primitives: Vec<Primitive>,
}
impl Tile {
    fn bytes(&self) -> usize {
        self.primitives
            .iter()
            .map(|p| {
                4 * (p.positions.len() + p.normals.len() + p.uvs.len() + p.indices.len())
                    + p.pixels.len()
            })
            .sum()
    }
}
#[derive(Default)]
pub struct Frame {
    pub loaded: Vec<Tile>,
    pub visible: Vec<u64>,
    pub removed: Vec<u64>,
    pub credits: Vec<String>,
    pub pending: u32,
    pub load_failed: bool,
    /// Every visible detail requirement is loaded and ready for rendering.
    pub selection_complete: bool,
    pub error: Option<&'static str>,
}
#[derive(Default, Debug)]
pub struct Statistics {
    pub requests: u64,
    pub transferred_bytes: u64,
    pub decoded_tiles: u64,
    pub failed_tiles: u64,
    pub resident_bytes: usize,
}
enum Completion {
    Root(Result<Vec<Node>>),
    Content(usize, Result<Payload>, usize),
}
enum Payload {
    Mesh(Tile, Vec<String>),
    Hierarchy(Vec<Node>),
}
/// Tile jobs in flight. Browser jobs mostly wait on the network; native jobs
/// each own a decoding thread.
const MAX_PENDING_TILE_JOBS: usize = if cfg!(target_arch = "wasm32") { 16 } else { 4 };
/// The remaining jobs stay free for prepared reads while the provider is slow.
const MAX_PROVIDER_TILE_JOBS: usize = MAX_PENDING_TILE_JOBS * 3 / 4;

/// The cache budget covers unused tiles, excluding the current visible and
/// refining working set. Work in flight is capped at sixteen tile jobs in the
/// browser and four on native; each response is limited to 32 MiB. This is not
/// a hard cap on total CPU/GPU memory.
pub struct Session {
    nodes: Vec<Node>,
    tx: mpsc::Sender<Completion>,
    rx: mpsc::Receiver<Completion>,
    pending: HashSet<usize>,
    failed: HashSet<usize>,
    retry_after: HashMap<usize, f64>,
    retry_attempts: HashMap<usize, u32>,
    failure_log: retry::FailureLog,
    elapsed: f64,
    resident: HashMap<usize, (usize, u64)>,
    render_ready: Option<HashSet<u64>>,
    tile_credits: HashMap<usize, Vec<String>>,
    clock: u64,
    root_pending: bool,
    root_failed: bool,
    client: transport::Loader,
    pub stats: Statistics,
    pub maximum_screen_error: f64,
    pub cache_bytes: usize,
}
impl Session {
    /// Prepared assets stay within their package directory. An ion token enables
    /// cache-first Photo 3D fallback using a sibling `package.json` manifest.
    pub fn open_prepared(source: &str, ion_token: Option<&str>) -> Result<Self> {
        let url = Url::parse(source).map_err(|_| Error::Message("Invalid prepared source URL"))?;
        let package = package::Package::new(&url, ion_token)?;
        Self::start(transport::Loader::prepared(package), Some(url))
    }

    pub fn open_terrain_url(source: &str, imagery: Option<terrain::Imagery>) -> Result<Self> {
        let url = Url::parse(source).map_err(|_| Error::Message("Invalid terrain URL"))?;
        if !matches!(url.scheme(), "https" | "http" | "file") {
            return Err(Error::Message("Unsupported source scheme"));
        }
        if let Some(i) = &imagery {
            i.url(terrain::TileAddress {
                level: 0,
                x: 0,
                y: 0,
            })?;
        }
        Self::start(
            transport::Loader::default().with_imagery(imagery),
            Some(url),
        )
    }
    pub fn open_url(source: &str) -> Result<Self> {
        let url = Url::parse(source).map_err(|_| Error::Message("Invalid source URL"))?;
        if !matches!(url.scheme(), "https" | "http" | "file") {
            return Err(Error::Message("Unsupported source scheme"));
        }
        Self::start(transport::Loader::default(), Some(url))
    }
    fn start(client: transport::Loader, url: Option<Url>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        client.load_root(url, tx.clone());
        Ok(Self {
            nodes: vec![],
            tx,
            rx,
            pending: HashSet::new(),
            failed: HashSet::new(),
            retry_after: HashMap::new(),
            retry_attempts: HashMap::new(),
            failure_log: retry::FailureLog::default(),
            elapsed: 0.0,
            resident: HashMap::new(),
            render_ready: None,
            tile_credits: HashMap::new(),
            clock: 0,
            root_pending: true,
            root_failed: false,
            client,
            stats: Statistics::default(),
            maximum_screen_error: 16.,
            cache_bytes: 256 * 1024 * 1024,
        })
    }
    pub fn open_ion(asset_id: i64, token: &str) -> Result<Self> {
        Self::start(
            transport::Loader::ion(ion::Ion::new(asset_id, token)?),
            None,
        )
    }
    /// Report render-prepared tiles before selection. Without feedback, decoded
    /// content is considered ready (for consumers without a separate renderer).
    pub fn set_render_ready(&mut self, ids: impl IntoIterator<Item = u64>) {
        let ready = self.render_ready.get_or_insert_default();
        ready.clear();
        ready.extend(ids);
    }

    pub fn update(&mut self, views: &[View], seconds: f32) -> Result<Frame> {
        if !seconds.is_finite()
            || seconds < 0.
            || views.iter().any(|v| !v.valid())
            || !self.maximum_screen_error.is_finite()
            || self.maximum_screen_error <= 0.
        {
            return Err(Error::Message("Invalid camera or screen error"));
        }
        self.clock += 1;
        self.elapsed += f64::from(seconds);
        self.retry_after.retain(|id, deadline| {
            if self.elapsed >= *deadline {
                self.failed.remove(id);
                false
            } else {
                true
            }
        });
        let mut frame = Frame {
            credits: self.client.credits(),
            ..Frame::default()
        };
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Completion::Root(result) => {
                    self.root_pending = false;
                    match result {
                        Ok(nodes) => self.nodes = nodes,
                        Err(e) => {
                            frame.error = Some(e.message());
                            eprintln!("3D Tiles: {e}");
                            self.root_failed = true;
                        }
                    }
                }
                Completion::Content(id, result, bytes) => {
                    self.pending.remove(&id);
                    self.stats.transferred_bytes += bytes as u64;
                    match result {
                        Ok(Payload::Mesh(tile, credits)) => {
                            self.retry_attempts.remove(&id);
                            self.tile_credits.insert(id, credits);
                            self.stats.decoded_tiles += 1;
                            self.resident.insert(id, (tile.bytes(), self.clock));
                            frame.loaded.push(tile);
                        }
                        Ok(Payload::Hierarchy(mut nodes)) => {
                            self.retry_attempts.remove(&id);
                            if self.nodes.len() + nodes.len() > 100_000 || nodes.is_empty() {
                                self.failed.insert(id);
                                self.stats.failed_tiles += 1;
                                frame.error = Some(if nodes.is_empty() {
                                    "Empty external tileset"
                                } else {
                                    "Tileset hierarchy limit exceeded"
                                });
                                continue;
                            }
                            let offset = self.nodes.len();
                            for node in &mut nodes {
                                for child in &mut node.children {
                                    *child += offset;
                                }
                            }
                            self.nodes[id].url = None;
                            self.nodes[id].children.push(offset);
                            self.nodes.extend(nodes);
                        }
                        Err(e) => {
                            frame.error = Some(e.message());
                            self.failure_log.record(id, e);
                            self.failed.insert(id);
                            let attempt = self.retry_attempts.get(&id).copied().unwrap_or(0);
                            if let Some(delay) = retry::delay(e, attempt, id) {
                                self.retry_after.insert(id, self.elapsed + delay);
                                self.retry_attempts.insert(id, attempt.saturating_add(1));
                            }
                            self.stats.failed_tiles += 1;
                        }
                    }
                }
            }
        }
        if let Some((count, id, error)) = self.failure_log.take(self.elapsed) {
            eprintln!("3D Tiles: {count} tile failure(s); example tile {id}: {error}");
        }
        let mut wanted = Vec::new();
        let mut required = HashSet::new();
        if !views.is_empty() && !self.nodes.is_empty() {
            self.select(
                0,
                views,
                &mut wanted,
                &mut frame.visible,
                &mut required,
                false,
            );
            if wanted.iter().any(|id| {
                self.nodes[*id]
                    .url
                    .as_ref()
                    .is_some_and(|url| self.client.requires_provider(url))
            }) {
                // A missing provider sibling must not hide prepared detail behind
                // a coarse ancestor. Select the available package as an offline
                // fallback while continuing to request the full hierarchy.
                let mut cached_visible = Vec::new();
                let mut cached_wanted = Vec::new();
                self.select(
                    0,
                    views,
                    &mut cached_wanted,
                    &mut cached_visible,
                    &mut required,
                    true,
                );
                if !cached_visible.is_empty() {
                    frame.visible = cached_visible;
                }
                cached_wanted.extend(wanted);
                wanted = cached_wanted;
            }
        }
        for id in &required {
            if let Some(r) = self.resident.get_mut(id) {
                r.1 = self.clock;
            }
        }
        let selection_complete = wanted.is_empty();
        // Prepared reads take priority. Within each source, request coarse
        // coverage before detail, then nearby bounds first. Reserve a slot for
        // prepared reads even while provider requests hang on later camera moves.
        self.prioritize_requests(&mut wanted, views);
        let mut provider_pending = self
            .pending
            .iter()
            .filter(|id| {
                self.nodes[**id]
                    .url
                    .as_ref()
                    .is_some_and(|url| self.client.requires_provider(url))
            })
            .count();
        for id in wanted {
            if self.pending.len() >= MAX_PENDING_TILE_JOBS {
                break;
            }
            if self.resident.contains_key(&id)
                || self.pending.contains(&id)
                || self.failed.contains(&id)
            {
                continue;
            }
            let Some(url) = self.nodes[id].url.clone() else {
                continue;
            };
            if self.client.requires_provider(&url) {
                if provider_pending >= MAX_PROVIDER_TILE_JOBS {
                    continue;
                }
                provider_pending += 1;
            }
            self.pending.insert(id);
            self.stats.requests += 1;
            self.client.load_tile(
                id,
                url,
                self.nodes[id].transform,
                self.nodes[id].add,
                self.nodes[id].depth,
                self.nodes[id].terrain,
                self.tx.clone(),
            );
        }
        let mut bytes: usize = self.resident.values().map(|v| v.0).sum();
        let mut candidates: Vec<_> = self
            .resident
            .iter()
            .filter(|(id, _)| !required.contains(*id))
            .map(|(id, (size, last))| (*last, *id, *size))
            .collect();
        candidates.sort_unstable();
        let mut unused_bytes: usize = candidates.iter().map(|(_, _, size)| size).sum();
        for (_, id, size) in candidates {
            if unused_bytes <= self.cache_bytes {
                break;
            }
            self.resident.remove(&id);
            self.tile_credits.remove(&id);
            unused_bytes -= size;
            bytes -= size;
            frame.removed.push(id as u64);
        }
        self.stats.resident_bytes = bytes;
        frame.pending = self.pending.len() as u32 + u32::from(self.root_pending);
        frame.load_failed = self.root_failed || !self.failed.is_empty();
        frame.selection_complete = selection_complete
            && frame.pending == 0
            && !frame.load_failed
            && !frame.visible.is_empty();
        let mut credits = HashMap::<&str, usize>::new();
        for id in &frame.visible {
            if let Some(items) = self.tile_credits.get(&(*id as usize)) {
                for item in items {
                    *credits.entry(item).or_default() += 1;
                }
            }
        }
        let mut credits: Vec<_> = credits.into_iter().collect();
        credits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        frame
            .credits
            .extend(credits.into_iter().map(|(c, _)| html_escape(c)));
        Ok(frame)
    }
    fn prioritize_requests(&self, wanted: &mut [usize], views: &[View]) {
        // Score once per tile, using only cameras that can see its bounds.
        let mut ranked: Vec<_> = wanted
            .iter()
            .map(|&id| {
                let node = &self.nodes[id];
                let provider = node
                    .url
                    .as_ref()
                    .is_some_and(|url| self.client.requires_provider(url));
                let distance = views
                    .iter()
                    .filter(|v| node.screen_error(v).is_some())
                    .map(|v| {
                        (DVec3::from_array(v.position).distance(node.center) - node.radius).max(0.0)
                    })
                    .fold(f64::INFINITY, f64::min);
                (id, (provider, node.depth, distance))
            })
            .collect();
        ranked
            .sort_by(|(_, a), (_, b)| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2)));
        for (target, (id, _)) in wanted.iter_mut().zip(ranked) {
            *target = id;
        }
    }

    fn select(
        &self,
        id: usize,
        views: &[View],
        wanted: &mut Vec<usize>,
        visible: &mut Vec<u64>,
        required: &mut HashSet<usize>,
        prepared_only: bool,
    ) -> bool {
        let n = &self.nodes[id];
        if prepared_only
            && n.url
                .as_ref()
                .is_some_and(|url| self.client.requires_provider(url))
        {
            return true;
        }
        let score = views
            .iter()
            .filter_map(|v| n.screen_error(v))
            .fold(None, |s: Option<f64>, e| Some(s.unwrap_or(0.).max(e)));
        let Some(error) = score else { return true };
        let has = n.url.is_some();
        let loaded = self.resident.contains_key(&id);
        let ready = loaded
            && self
                .render_ready
                .as_ref()
                .is_none_or(|set| set.contains(&(id as u64)));
        // Decoded descendants waiting for render preparation are part of the
        // refining working set, even while a coarse ancestor covers them.
        if loaded && !ready {
            required.insert(id);
        }
        let request_start = wanted.len();
        if !n.children.is_empty() && (!has || error > self.maximum_screen_error) {
            let before = visible.len();
            let mut all_ready = true;
            for child in &n.children {
                all_ready &= self.select(*child, views, wanted, visible, required, prepared_only);
            }
            if n.add {
                if ready {
                    required.insert(id);
                    visible.push(id as u64);
                } else if has {
                    wanted.insert(request_start, id);
                }
                return (!has || ready) && all_ready;
            }
            if all_ready && (!prepared_only || visible.len() > before || !has) {
                return true;
            }
            if ready {
                // Keep ready descendants while this fallback covers siblings
                // still loading; evicting them here causes repeated downloads.
                required.insert(id);
                visible.truncate(before);
                visible.push(id as u64);
                return true;
            }
            if has {
                wanted.insert(request_start, id);
            }
            return false;
        }
        if ready {
            required.insert(id);
            visible.push(id as u64);
        } else if has {
            // On zoom-out, keep available detail until the coarser replacement
            // is render-ready. Do not request extra detail just for this fallback.
            if !n.add {
                let before = visible.len();
                let mut all_ready = !n.children.is_empty();
                for child in &n.children {
                    all_ready &=
                        self.select_resident(*child, views, visible, required, prepared_only);
                }
                if loaded && all_ready && visible.len() > before {
                    return true;
                }
            }
            wanted.push(id);
        }
        ready || !has
    }

    fn select_resident(
        &self,
        id: usize,
        views: &[View],
        visible: &mut Vec<u64>,
        required: &mut HashSet<usize>,
        prepared_only: bool,
    ) -> bool {
        let node = &self.nodes[id];
        if !views.iter().any(|v| node.screen_error(v).is_some())
            || (prepared_only
                && node
                    .url
                    .as_ref()
                    .is_some_and(|url| self.client.requires_provider(url)))
        {
            return true;
        }
        let mut ready = false;
        if self.resident.contains_key(&id) {
            required.insert(id);
            if self
                .render_ready
                .as_ref()
                .is_none_or(|set| set.contains(&(id as u64)))
            {
                ready = true;
                visible.push(id as u64);
                if !node.add {
                    return true;
                }
            }
        }
        // A partial set of resident descendants is useful while loading, but
        // must not make an ancestor drop its complete fallback coverage.
        let mut all_ready = true;
        for child in &node.children {
            all_ready &= self.select_resident(*child, views, visible, required, prepared_only);
        }
        all_ready && (node.url.is_none() || ready || (!node.add && !node.children.is_empty()))
    }
}

pub(crate) fn resolve(base: &Url, reference: &str) -> Result<Url> {
    let mut url = base
        .join(reference)
        .map_err(|_| Error::Message("Invalid resource URL"))?;
    if !matches!(url.scheme(), "http" | "https" | "file")
        || (url.scheme() == "file" && base.scheme() != "file")
    {
        return Err(Error::Message("Resource scheme not allowed"));
    }
    if google_url(base) && google_url(&url) {
        for key in ["key", "session"] {
            if !url.query_pairs().any(|(k, _)| k == key)
                && let Some((_, value)) = base.query_pairs().find(|(k, _)| k == key)
            {
                url.query_pairs_mut().append_pair(key, &value);
            }
        }
    }
    Ok(url)
}

pub(crate) fn google_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("tile.googleapis.com")
        && url.port_or_known_default() == Some(443)
        && url.path().starts_with("/v1/3dtiles/")
}
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn parse_root(bytes: &[u8], base: &Url) -> Result<Vec<Node>> {
    let json: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| Error::Message("Invalid source manifest"))?;
    if json.get("format").and_then(|s| s.as_str()) == Some("quantized-mesh-1.0") {
        terrain::layer(bytes, base)
    } else {
        parse_tileset(bytes, base)
    }
}
fn root_credits(bytes: &[u8], url: &Url) -> Vec<String> {
    let mut result = Vec::new();
    if google_url(url) {
        result.push("<a href=\"https://maps.google.com\">Google Maps</a>".into());
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes)
        && let Some(credit) = value.get("attribution").and_then(|v| v.as_str())
    {
        result.push(credit.into());
    }
    result
}
