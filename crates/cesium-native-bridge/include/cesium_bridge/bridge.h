#pragma once
#include "rust/cxx.h"
#include <memory>

namespace cesium_bridge {
struct View;
struct Frame;
struct CartographicPosition;
class Session {
public:
  explicit Session(rust::Str url);
  Session(int64_t asset, rust::Str token, bool imagery = true);
  ~Session();
  void configure(double error, uint64_t cacheBytes);
  void configure_retention(double marginMeters);
  Frame update(rust::Slice<const View> views, float seconds);
  rust::Vec<double> sample_heights(rust::Slice<const CartographicPosition> positions, uint64_t timeoutMillis);
private:
  struct Impl;
  std::unique_ptr<Impl> impl;
};
std::unique_ptr<Session> open_url(rust::Str url);
std::unique_ptr<Session> open_ion(int64_t asset, rust::Str token);
std::unique_ptr<Session> open_terrain_ion(int64_t asset, rust::Str token);
}
