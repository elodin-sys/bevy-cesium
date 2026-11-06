use bevy_cesium::native::{CartographicPosition, Session, View};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[test]
fn streaming_and_heights_reuse_and_extend_the_cache_across_processes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(AtomicBool::new(false));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let cache = std::env::temp_dir().join(format!("cesium-cache-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&cache).unwrap();
    let blocked = cache.join("not-a-directory");
    std::fs::write(&blocked, "fixture").unwrap();
    thread::scope(|scope| {
        let server = scope.spawn(|| {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 { break; }
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap().to_owned();
                requests.lock().unwrap().push(path.clone());
                let body: &[u8] = if path.ends_with("height.json") {
                    include_bytes!("data/height.json")
                } else if path.ends_with("height.gltf") {
                    include_bytes!("data/height.gltf")
                } else { panic!("Unexpected cache-test request: {path}"); };
                let policy = if path.contains("uncached") { "no-store" } else { "public, max-age=3600" };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: {policy}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(body).unwrap();
            }
        });
        // Always shut the server down before propagating an assertion panic.
        let result = std::panic::catch_unwind(|| {
            let run = |area: &str, mode: &str| {
                let url = format!("http://{address}/{nonce}/{area}/height.json");
                let output = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "cache_worker", "--ignored", "--nocapture"])
                    .env(
                        "XDG_CACHE_HOME",
                        if area == "unavailable" {
                            &blocked
                        } else {
                            &cache
                        },
                    )
                    .env("BEVY_CESIUM_CACHE_TEST_URL", url)
                    .env("BEVY_CESIUM_CACHE_TEST_MODE", mode)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            run("first", "stream");
            assert_eq!(requests.lock().unwrap().len(), 2);
            assert!(cache.join("bevy-cesium/terrain/requests.sqlite").is_file());
            run("first", "heights");
            assert_eq!(
                requests.lock().unwrap().len(),
                2,
                "A new process must reuse both cached responses"
            );
            run("second", "heights");
            assert_eq!(
                requests.lock().unwrap().len(),
                4,
                "A new region must be fetched on demand"
            );
            run("first", "stream");
            assert_eq!(
                requests.lock().unwrap().len(),
                4,
                "Adding a region must retain the earlier region"
            );
            run("uncached", "stream");
            run("uncached", "heights");
            assert_eq!(
                requests.lock().unwrap().len(),
                8,
                "Provider no-store responses must not be cached"
            );
            run("unavailable", "stream");
            run("unavailable", "heights");
            assert_eq!(
                requests.lock().unwrap().len(),
                12,
                "An unavailable cache must not block streaming or heights"
            );
        });
        stopped.store(true, Ordering::Relaxed);
        server.join().unwrap();
        std::fs::remove_dir_all(&cache).unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    });
}

#[test]
#[ignore = "subprocess helper invoked by the persistent-cache test"]
fn cache_worker() {
    let url = std::env::var("BEVY_CESIUM_CACHE_TEST_URL").unwrap();
    let mut session = Session::open_url(&url).unwrap();
    if std::env::var("BEVY_CESIUM_CACHE_TEST_MODE").unwrap() == "heights" {
        let heights = session
            .sample_heights(
                &[CartographicPosition {
                    longitude_radians: 2.0 / 6_378_137.0,
                    latitude_radians: 2.0 / 6_335_439.0,
                }],
                Duration::from_secs(10),
            )
            .unwrap();
        assert!(heights[0].abs() < 0.01);
    } else {
        let view = View {
            position: [6_378_200.0, 5.0, -5.0],
            direction: [-1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            width: 1280.0,
            height: 720.0,
            horizontal_fov: 1.4,
            vertical_fov: 0.9,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let frame = session.update(&[view], 0.02).unwrap();
            assert!(!frame.load_failed);
            if !frame.visible.is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("Fixture did not stream");
    }
}
