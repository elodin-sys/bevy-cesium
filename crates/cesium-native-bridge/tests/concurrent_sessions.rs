use cesium_native_bridge::{Session, View};
use std::{
    sync::Barrier,
    time::{Duration, Instant},
};

#[test]
fn independent_threads_can_initialize_and_stream_sessions_together() {
    let start = Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                start.wait();
                let mut session = Session::open_url(&format!(
                    "file://{}/tests/data/tileset.json",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap();
                let view = View {
                    position: [6_378_200.0, 5.0, -5.0],
                    direction: [-1.0, 0.0, 0.0],
                    up: [0.0, 0.0, 1.0],
                    width: 1280.0,
                    height: 720.0,
                    horizontal_fov: 1.4,
                    vertical_fov: 0.9,
                };
                let deadline = Instant::now() + Duration::from_secs(15);
                while Instant::now() < deadline {
                    let frame = session.update(&[view], 0.02).unwrap();
                    assert!(!frame.load_failed);
                    if !frame.visible.is_empty() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                panic!("Concurrent session did not render its tile");
            });
        }
    });
}
