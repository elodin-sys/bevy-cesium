#include "bevy-cesium/src/native.rs.h"
#include <Cesium3DTilesContent/registerAllTileContentTypes.h>
#include <Cesium3DTilesSelection/IPrepareRendererResources.h>
#include <Cesium3DTilesSelection/Tileset.h>
#include <Cesium3DTilesSelection/ViewState.h>
#include <Cesium3DTilesSelection/ViewUpdateResult.h>
#include <Cesium3DTilesSelection/RasterOverlayCollection.h>
#include <CesiumRasterOverlays/IonRasterOverlay.h>
#include <CesiumAsync/ITaskProcessor.h>
#include <CesiumCurl/CurlAssetAccessor.h>
#include <CesiumGltf/AccessorUtility.h>
#include <CesiumGltfContent/GltfUtilities.h>
#include <CesiumUtility/CreditSystem.h>
#include <glm/gtc/type_ptr.hpp>
#include <spdlog/sinks/null_sink.h>
#include <condition_variable>
#include <atomic>
#include <cstdlib>
#include <cmath>
#include <deque>
#include <mutex>
#include <thread>
#include <charconv>
#include <unordered_map>
#include <unordered_set>
#include <limits>
#include <algorithm>

namespace cesium_bridge {
namespace {
using namespace Cesium3DTilesSelection;

// Geographic selection views preserve the refinement path to previously drawn
// tiles. They do not change dataset errors, freeze cameras, or request arbitrary
// maximum detail. Live views remain free to refine further.
class RetainedMeasure final : public ViewStateMeasureDelegate {
public:
  std::unordered_set<const Cesium3DTilesSelection::Tile*> ancestors;
  double computeSelectionMeasure(const Cesium3DTilesSelection::Tile& tile,
      double, uint32_t) const override {
    return ancestors.contains(&tile) ? std::numeric_limits<double>::max() : 0.0;
  }
};

class DetailRetention {
  using NativeTile = Cesium3DTilesSelection::Tile;
  std::unordered_map<const NativeTile*, NativeTile::ConstPointer> tiles;
public:
  double margin = 0.0;
  void clear() { tiles.clear(); }
  size_t size() const { return tiles.size(); }

  void appendViews(std::vector<ViewState>& views) {
    if (margin <= 0.0) return;
    // No cameras is a scheduling gap, not proof that terrain is no longer
    // needed. An explicit configure_retention(0) releases it immediately.
    if (!views.empty()) {
      std::erase_if(tiles, [&](const auto& entry) {
        const auto box = getOrientedBoundingBoxFromBoundingVolume(entry.first->getBoundingVolume());
        const auto axes = box.getHalfAxes();
        const double radius = glm::length(axes[0]) + glm::length(axes[1]) + glm::length(axes[2]) + margin;
        const BoundingVolume expanded = CesiumGeometry::BoundingSphere(box.getCenter(), radius);
        return std::none_of(views.begin(), views.end(), [&](const ViewState& view) {
          return view.isBoundingVolumeVisible(expanded);
        });
      });
    }
    if (tiles.empty()) return;
    auto measure = std::make_shared<RetainedMeasure>();
    glm::dvec3 minimum(std::numeric_limits<double>::max());
    glm::dvec3 maximum(std::numeric_limits<double>::lowest());
    for (const auto& [tile, reference] : tiles) {
      for (auto* parent = tile->getParent(); parent; parent = parent->getParent())
        measure->ancestors.insert(parent);
      const auto box = getOrientedBoundingBoxFromBoundingVolume(tile->getBoundingVolume());
      const auto axes = box.getHalfAxes();
      const auto extent = glm::abs(axes[0]) + glm::abs(axes[1]) + glm::abs(axes[2]);
      minimum = glm::min(minimum, box.getCenter() - extent);
      maximum = glm::max(maximum, box.getCenter() + extent);
    }
    // Native tests every visited tile against every selection view. One view
    // per retained tile turns this into a quadratic traversal. A single
    // enclosing volume keeps the same forced ancestor paths at a bounded cost;
    // unrelated paths still have a zero retention selection measure. The
    // enclosure may also select coarse tiles in gaps between retained areas.
    const auto center = (minimum + maximum) * 0.5;
    const auto half = (maximum - minimum) * 0.5;
    views.emplace_back(CesiumGeometry::OrientedBoundingBox(center, glm::dmat3(
        glm::dvec3(half.x, 0.0, 0.0), glm::dvec3(0.0, half.y, 0.0),
        glm::dvec3(0.0, 0.0, half.z))), measure);
  }

  void observe(const ViewUpdateResult& result, const std::vector<ViewState>& live,
      double maximumScreenError) {
    if (margin <= 0.0) return;
    for (const auto& tile : result.tilesToRenderThisFrame) {
      const auto* content = tile->getContent().getRenderContent();
      if (!content || !content->getRenderResources()) continue;
      // Do not expand retention by recursively remembering tiles selected only
      // by retention views. Acquire detail only where real/reference cameras look.
      if (std::any_of(live.begin(), live.end(), [&](const ViewState& view) {
            if (!view.isBoundingVolumeVisible(tile->getBoundingVolume())) return false;
            // A fine tile can be a temporary fallback while a newly requested
            // coarse parent loads. Do not reacquire that released detail unless
            // a real camera actually requires refinement of its parent.
            const auto* parent = tile->getParent();
            return !parent || view.computeScreenSpaceError(parent->getGeometricError(),
                std::sqrt(view.computeDistanceSquaredToBoundingVolume(parent->getBoundingVolume())))
                > maximumScreenError;
          }))
        tiles.try_emplace(tile.get(), tile);
    }
    // A finer retained tile already requires descent through its ancestors.
    // Drop their broad volumes so a globe-sized root cannot pin a whole journey.
    std::unordered_set<const NativeTile*> superseded;
    for (const auto& [tile, reference] : tiles)
      for (auto* parent = tile->getParent(); parent; parent = parent->getParent())
        superseded.insert(parent);
    for (const auto* tile : superseded) tiles.erase(tile);
  }
};

void registerContentTypes() {
  // Native's process-wide converter registry must not be mutated while another
  // session is decoding a tile, including sessions owned by different threads.
  static std::once_flag registered;
  std::call_once(registered, Cesium3DTilesContent::registerAllTileContentTypes);
}

// Bounded worker count; destruction drains work and joins every worker.
class Workers final : public CesiumAsync::ITaskProcessor {
  std::mutex mutex;
  std::condition_variable wake;
  std::deque<std::function<void()>> jobs;
  bool stopping = false;
  std::vector<std::thread> threads;
public:
  Workers() {
    for (int i = 0; i < 4; ++i) threads.emplace_back([this] {
      for (;;) {
        std::function<void()> job;
        {
          std::unique_lock lock(mutex);
          wake.wait(lock, [this] { return stopping || !jobs.empty(); });
          if (jobs.empty() && stopping) return;
          job = std::move(jobs.front()); jobs.pop_front();
        }
        job();
      }
    });
  }
  ~Workers() override {
    { std::lock_guard lock(mutex); stopping = true; }
    wake.notify_all();
    for (auto& thread : threads) thread.join();
  }
  void startTask(std::function<void()> task) override {
    { std::lock_guard lock(mutex); jobs.push_back(std::move(task)); }
    wake.notify_one();
  }
};

struct Raster {
  uint32_t width = 0, height = 0;
  std::vector<uint8_t> pixels;
};

Raster extractRaster(const CesiumImage::ImageAsset& image) {
  Raster raster;
  if (image.width <= 0 || image.height <= 0 || image.bytesPerChannel != 1 ||
      image.channels < 1 || image.channels > 4 ||
      image.compressedPixelFormat != CesiumImage::GpuCompressedPixelFormat::NONE) return raster;
  const auto count = size_t(image.width) * size_t(image.height);
  if (image.pixelData.size() < count * image.channels) return raster;
  raster.width = image.width; raster.height = image.height;
  raster.pixels.reserve(count * 4);
  for (size_t i = 0; i < count; ++i) {
    auto read = [&](int c) { return uint8_t(image.pixelData[i * image.channels + c]); };
    const bool gray = image.channels < 3;
    raster.pixels.push_back(read(0));
    raster.pixels.push_back(read(gray ? 0 : 1));
    raster.pixels.push_back(read(gray ? 0 : 2));
    raster.pixels.push_back(image.channels == 2 ? read(1) : image.channels == 4 ? read(3) : 255);
  }
  return raster;
}

Tile extract(const CesiumGltf::Model& model, const glm::dmat4& tileTransform,
    const Raster* raster = nullptr, int32_t overlayCoordinate = 0,
    glm::dvec2 translation = glm::dvec2(0.0), glm::dvec2 scale = glm::dvec2(1.0)) {
  Tile output{};
  const auto root = CesiumGltfContent::GltfUtilities::applyGltfUpAxisTransform(
      model, CesiumGltfContent::GltfUtilities::applyRtcCenter(model, tileTransform));
  model.forEachPrimitiveInScene(-1, [&](const CesiumGltf::Model& gltf,
      const CesiumGltf::Node&, const CesiumGltf::Mesh&,
      const CesiumGltf::MeshPrimitive& primitive, const glm::dmat4& node) {
    if (primitive.mode != 4) return; // First prototype: triangle surfaces.
    const auto positions = CesiumGltf::getPositionAccessorView(gltf, primitive);
    const auto normals = CesiumGltf::getNormalAccessorView(gltf, primitive);
    int32_t texCoord = 0;
    if (primitive.material >= 0 && size_t(primitive.material) < gltf.materials.size()) {
      const auto& material = gltf.materials[primitive.material];
      if (material.pbrMetallicRoughness && material.pbrMetallicRoughness->baseColorTexture)
        texCoord = material.pbrMetallicRoughness->baseColorTexture->texCoord;
    }
    auto texturedPrimitive = primitive;
    if (raster) {
      auto attribute = primitive.attributes.find("_CESIUMOVERLAY_" + std::to_string(overlayCoordinate));
      if (attribute == primitive.attributes.end()) return;
      texturedPrimitive.attributes["TEXCOORD_0"] = attribute->second;
      texCoord = 0;
    }
    const auto uvs = CesiumGltf::getTexCoordAccessorView(gltf, texturedPrimitive, texCoord);
    const auto count = std::visit(CesiumGltf::CountFromAccessor{}, positions);
    if (count <= 0) return;
    Primitive p{};
    auto matrix = root * node;
    std::copy_n(glm::value_ptr(matrix), 16, p.transform.begin());
    p.base_color = {1.f, 1.f, 1.f, 1.f};
    p.metallic = 1.f; p.roughness = 1.f; p.alpha_cutoff = 0.5f;
    for (int64_t i = 0; i < count; ++i) {
      const auto position = std::visit(CesiumGltf::PositionFromAccessor{i}, positions);
      if (!position) return;
      const auto normal = std::visit(CesiumGltf::NormalFromAccessor{i}, normals);
      const auto uv = std::visit(CesiumGltf::TexCoordFromAccessor{i}, uvs);
      for (int axis = 0; axis < 3; ++axis) {
        p.positions.push_back(float((*position)[axis]));
        p.normals.push_back(normal ? float((*normal)[axis]) : float(axis == 1));
      }
      const auto mapped = (uv ? *uv : glm::dvec2(0.0)) * scale + translation;
      p.uvs.push_back(float(mapped.x));
      // Native raster UVs have their origin at the bottom left; Bevy images
      // use top-left texture coordinates.
      p.uvs.push_back(float(raster ? 1.0 - mapped.y : mapped.y));
    }
    if (primitive.indices >= 0) {
      auto indices = CesiumGltf::getIndexAccessorView(gltf, primitive);
      auto n = std::visit(CesiumGltf::CountFromAccessor{}, indices);
      for (int64_t i = 0; i < n; ++i) {
        auto index = std::visit(CesiumGltf::IndexFromAccessor{i}, indices);
        if (index < 0 || index >= count) return;
        p.indices.push_back(uint32_t(index));
      }
    } else {
      for (int64_t i = 0; i < count; ++i) p.indices.push_back(uint32_t(i));
    }
    if (p.indices.size() % 3 != 0) return;
    if (primitive.material >= 0 && size_t(primitive.material) < gltf.materials.size()) {
      const auto& material = gltf.materials[primitive.material];
      p.double_sided = material.doubleSided;
      p.unlit = material.extensions.contains("KHR_materials_unlit");
      p.alpha_mode = material.alphaMode == "MASK" ? 1 : material.alphaMode == "BLEND" ? 2 : 0;
      p.alpha_cutoff = float(material.alphaCutoff);
      if (material.emissiveFactor.size() == 3)
        for (int i = 0; i < 3; ++i) p.emissive[i] = float(material.emissiveFactor[i]);
      if (material.pbrMetallicRoughness) {
        const auto& pbr = *material.pbrMetallicRoughness;
        p.metallic = float(pbr.metallicFactor);
        p.roughness = float(pbr.roughnessFactor);
        if (pbr.baseColorFactor.size() == 4)
          for (int i = 0; i < 4; ++i) p.base_color[i] = float(pbr.baseColorFactor[i]);
        if (pbr.baseColorTexture && pbr.baseColorTexture->index >= 0 &&
            size_t(pbr.baseColorTexture->index) < gltf.textures.size()) {
          auto source = gltf.textures[pbr.baseColorTexture->index].source;
          if (source >= 0 && size_t(source) < gltf.images.size()) {
            auto image = gltf.images[source].pAsset;
            if (image) {
              const auto decoded = extractRaster(*image);
              p.width = decoded.width; p.height = decoded.height;
              p.pixels = copy_pixels(rust::Slice<const uint8_t>(decoded.pixels.data(), decoded.pixels.size()));
            }
          }
        }
      }
    }
    if (raster) {
      p.base_color = {1.f, 1.f, 1.f, 1.f};
      p.metallic = 0.f; p.roughness = 1.f;
      p.width = raster->width; p.height = raster->height;
      p.pixels = copy_pixels(rust::Slice<const uint8_t>(raster->pixels.data(), raster->pixels.size()));
    }
    output.primitives.push_back(std::move(p));
  });
  return output;
}

class Resources final : public IPrepareRendererResources {
public:
  uint64_t nextId = 1;
  rust::Vec<Tile> loaded;
  rust::Vec<uint64_t> removed;
  std::unordered_map<uint64_t, const Raster*> attached;
  CesiumAsync::Future<TileLoadResultAndRenderResources> prepareInLoadThread(
      const CesiumAsync::AsyncSystem& async, TileLoadResult&& result,
      const glm::dmat4& transform, const std::any&) override {
    auto packet = std::make_unique<Tile>();
    if (auto* model = std::get_if<CesiumGltf::Model>(&result.contentKind)) {
      model->generateMissingNormalsSmooth();
      *packet = extract(*model, transform);
    }
    return async.createResolvedFuture(TileLoadResultAndRenderResources{std::move(result), packet.release()});
  }
  void* prepareInMainThread(Cesium3DTilesSelection::Tile&, void* data) override {
    std::unique_ptr<Tile> packet(static_cast<Tile*>(data));
    auto id = nextId++;
    packet->id = id;
    loaded.push_back(std::move(*packet));
    return new uint64_t(id);
  }
  void free(Cesium3DTilesSelection::Tile&, void* loading, void* ready) noexcept override {
    delete static_cast<Tile*>(loading);
    if (ready) {
      auto* id = static_cast<uint64_t*>(ready);
      removed.push_back(*id);
      attached.erase(*id);
      delete id;
    }
  }
  void* prepareRasterInLoadThread(CesiumImage::ImageAsset& image, const std::any&) override {
    return new Raster(extractRaster(image));
  }
  void* prepareRasterInMainThread(CesiumRasterOverlays::RasterOverlayTile&, void* data) override {
    return data;
  }
  void freeRaster(const CesiumRasterOverlays::RasterOverlayTile&, void* loading, void* ready) noexcept override {
    delete static_cast<Raster*>(loading);
    delete static_cast<Raster*>(ready);
  }
  void attachRasterInMainThread(const Cesium3DTilesSelection::Tile& tile, int32_t coordinate,
      const CesiumRasterOverlays::RasterOverlayTile&, void* data,
      const glm::dvec2& translation, const glm::dvec2& scale) override {
    const auto* raster = static_cast<const Raster*>(data);
    const auto* content = tile.getContent().getRenderContent();
    if (!raster || raster->pixels.empty() || !content || !content->getRenderResources()) return;
    const auto id = *static_cast<const uint64_t*>(content->getRenderResources());
    auto packet = extract(content->getModel(), tile.getTransform(), raster, coordinate, translation, scale);
    packet.id = id;
    attached[id] = raster;
    loaded.push_back(std::move(packet));
  }
  void detachRasterInMainThread(const Cesium3DTilesSelection::Tile& tile, int32_t,
      const CesiumRasterOverlays::RasterOverlayTile&, void* data) noexcept override {
    const auto* content = tile.getContent().getRenderContent();
    if (!content || !content->getRenderResources()) return;
    const auto id = *static_cast<const uint64_t*>(content->getRenderResources());
    auto previous = attached.find(id);
    if (previous == attached.end() || previous->second != data) return;
    attached.erase(previous);
    // The next attachment replaces the texture. Keep the last good image
    // while its replacement loads to avoid flashing untextured terrain.
  }
};
}

struct Session::Impl {
  std::shared_ptr<Workers> workers = std::make_shared<Workers>();
  CesiumAsync::AsyncSystem async{workers};
  static std::shared_ptr<CesiumCurl::CurlAssetAccessor> createAccessor() {
    CesiumCurl::CurlAssetAccessorOptions options;
    if (auto* file = std::getenv("SSL_CERT_FILE")) options.certificateFile = file;
    else if (auto* file = std::getenv("NIX_SSL_CERT_FILE")) options.certificateFile = file;
    return std::make_shared<CesiumCurl::CurlAssetAccessor>(options);
  }
  std::shared_ptr<CesiumCurl::CurlAssetAccessor> accessor = createAccessor();
  std::shared_ptr<Resources> resources = std::make_shared<Resources>();
  std::shared_ptr<CesiumUtility::CreditSystem> credits = std::make_shared<CesiumUtility::CreditSystem>();
  std::unique_ptr<Tileset> tileset;
  DetailRetention retention;
  std::shared_ptr<std::atomic<bool>> loadFailed = std::make_shared<std::atomic<bool>>(false);
  TilesetOptions options() {
    TilesetOptions options;
    // Load the requested views first. Unused relatives can otherwise consume
    // the cache and delay readiness for narrow-field sensor cameras.
    options.preloadAncestors = false;
    options.preloadSiblings = false;
    auto failed = loadFailed;
    // Native 0.65 calls this for root/ion failures, not failed child tiles.
    options.loadErrorCallback = [failed](const auto&) { failed->store(true); };
    return options;
  }
  TilesetExternals externals() {
    // Native errors may contain signed asset URLs. Never forward those to logs.
    auto logger = std::make_shared<spdlog::logger>("cesium", std::make_shared<spdlog::sinks::null_sink_mt>());
    return {accessor, resources, async, credits, logger};
  }
  void observeRootFailure() {
    auto failed = loadFailed;
    // Transport exceptions reject the root future without calling loadErrorCallback.
    tileset->getRootTileAvailableEvent().catchInMainThread(
        [failed](std::exception&&) { failed->store(true); });
  }
  ~Impl() {
    if (!tileset) return;
    // Intrusive references must be released before destroying their tileset.
    retention.clear();
    auto finished = tileset->getAsyncDestructionCompleteEvent();
    tileset.reset();
    while (!finished.isReady()) {
      accessor->tick(); async.dispatchMainThreadTasks();
      std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
  }
};
Session::Session(rust::Str url) : impl(std::make_unique<Impl>()) {
  registerContentTypes();
  impl->tileset = std::make_unique<Tileset>(impl->externals(), std::string(url), impl->options());
  impl->observeRootFailure();
}
Session::Session(int64_t asset, rust::Str token, bool imagery) : impl(std::make_unique<Impl>()) {
  registerContentTypes();
  impl->tileset = std::make_unique<Tileset>(impl->externals(), asset, std::string(token), impl->options());
  if (const auto* value = imagery ? std::getenv("CESIUM_ION_IMAGERY_ASSET_ID") : nullptr) {
    const std::string text(value);
    int64_t imagery = 0;
    const auto parsed = std::from_chars(text.data(), text.data() + text.size(), imagery);
    if (parsed.ec != std::errc{} || parsed.ptr != text.data() + text.size() || imagery <= 0)
      throw std::runtime_error("Invalid imagery asset ID");
    CesiumRasterOverlays::RasterOverlayOptions options;
    auto failed = impl->loadFailed;
    options.loadErrorCallback = [failed](const auto&) { failed->store(true); };
    impl->tileset->getOverlays().add(
        CesiumUtility::IntrusivePointer<CesiumRasterOverlays::IonRasterOverlay>(
            new CesiumRasterOverlays::IonRasterOverlay("Aerial imagery", imagery, std::string(token), options)));
  }
  impl->observeRootFailure();
}
Session::~Session() = default;
void Session::configure(double error, uint64_t cacheBytes) {
  impl->tileset->getOptions().maximumScreenSpaceError = error;
  impl->tileset->getOptions().maximumCachedBytes = static_cast<int64_t>(cacheBytes);
}
void Session::configure_retention(double marginMeters) {
  impl->retention.margin = marginMeters;
  if (marginMeters <= 0.0) impl->retention.clear();
}
std::unique_ptr<Session> open_url(rust::Str url) { return std::make_unique<Session>(url); }
std::unique_ptr<Session> open_ion(int64_t asset, rust::Str token) { return std::make_unique<Session>(asset, token); }
std::unique_ptr<Session> open_terrain_ion(int64_t asset, rust::Str token) { return std::make_unique<Session>(asset, token, false); }
rust::Vec<double> Session::sample_heights(rust::Slice<const CartographicPosition> input, uint64_t timeoutMillis) {
  std::vector<CesiumGeospatial::Cartographic> positions;
  positions.reserve(input.size());
  for (const auto& p : input)
    positions.emplace_back(p.longitude_radians, p.latitude_radians, 0.0);
  auto future = impl->tileset->sampleHeightMostDetailed(positions);
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(timeoutMillis);
  while (!future.isReady()) {
    if (std::chrono::steady_clock::now() >= deadline || impl->loadFailed->load())
      throw std::runtime_error("Terrain height query failed or timed out");
    impl->tileset->updateViewGroup(impl->tileset->getDefaultViewGroup(), {}, 0.0f);
    impl->tileset->loadTiles();
    impl->accessor->tick();
    impl->async.dispatchMainThreadTasks();
    // A height query does not need mesh copies for a renderer.
    impl->resources->loaded.clear();
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  const auto result = future.wait();
  if (result.positions.size() != input.size() || result.sampleSuccess.size() != input.size())
    throw std::runtime_error("Incomplete terrain height query");
  rust::Vec<double> heights;
  heights.reserve(input.size());
  for (size_t i = 0; i < input.size(); ++i) {
    if (!result.sampleSuccess[i] || !std::isfinite(result.positions[i].height))
      throw std::runtime_error("Missing terrain height");
    heights.push_back(result.positions[i].height);
  }
  return heights;
}
Frame Session::update(rust::Slice<const View> input, float seconds) {
  std::vector<ViewState> views;
  for (const auto& view : input) {
    auto vec = [](const std::array<double, 3>& a) { return glm::dvec3(a[0], a[1], a[2]); };
    views.emplace_back(vec(view.position), vec(view.direction), vec(view.up),
        glm::dvec2(view.width, view.height), view.horizontal_fov, view.vertical_fov);
  }
  impl->accessor->tick();
  impl->async.dispatchMainThreadTasks();
  auto& tileset = *impl->tileset;
  const auto live = views;
  impl->retention.appendViews(views);
  const auto& result = tileset.updateViewGroup(tileset.getDefaultViewGroup(), views, seconds);
  impl->retention.observe(result, live, tileset.getOptions().maximumScreenSpaceError);
  tileset.loadTiles();
  Frame frame{};
  frame.loaded = std::move(impl->resources->loaded);
  frame.removed = std::move(impl->resources->removed);
  for (const auto& tile : result.tilesToRenderThisFrame) {
    auto* content = tile->getContent().getRenderContent();
    if (content && content->getRenderResources())
      frame.visible.push_back(*static_cast<const uint64_t*>(content->getRenderResources()));
  }
  for (auto credit : impl->credits->getSnapshot().currentCredits)
    frame.credits.push_back(impl->credits->getHtml(credit));
  frame.pending = result.workerThreadTileLoadQueueLength + result.mainThreadTileLoadQueueLength;
  frame.retained_tiles = static_cast<uint32_t>(impl->retention.size());
  frame.load_failed = impl->loadFailed->load();
  bool failedContent = false;
  tileset.forEachLoadedTile([&](const Cesium3DTilesSelection::Tile& tile) {
    failedContent |= tile.getState() == TileLoadState::Failed ||
        tile.getState() == TileLoadState::FailedTemporarily;
  });
  frame.selection_complete = !frame.load_failed && !failedContent &&
      !frame.visible.empty() && tileset.computeLoadProgress() >= 100.0f;
  return frame;
}
}
