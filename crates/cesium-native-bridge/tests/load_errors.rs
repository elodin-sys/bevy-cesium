use cesium_native_bridge::{Session, View};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct Server {
    url: String,
    missing: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let missing = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let missing = missing.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("fixture server failed: {error}"),
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                        let size = stream.read(&mut buffer).unwrap();
                        if size == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..size]);
                        assert!(request.len() < 16384);
                    }
                    let request = String::from_utf8_lossy(&request);
                    let path = request.split_whitespace().nth(1).unwrap_or("");
                    let (status, body) = match path {
                        "/missing-child.json" => {
                            ("200 OK", include_str!("data/missing-child.json"))
                        }
                        "/triangle.gltf" => ("200 OK", include_str!("data/triangle.gltf")),
                        _ => {
                            missing.fetch_add(1, Ordering::Relaxed);
                            ("404 Not Found", "missing")
                        }
                    };
                    write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            })
        };
        Self {
            url,
            missing,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn view() -> View {
    View {
        position: [6_378_200.0, 5.0, -5.0],
        direction: [-1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        width: 1280.0,
        height: 720.0,
        horizontal_fov: 1.4,
        vertical_fov: 0.9,
    }
}
fn open(file: &str) -> Session {
    Session::open_url(&format!(
        "file://{}/tests/data/{file}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
#[test]
fn missing_root_reports_a_tileset_failure() {
    let mut session = open("missing-root.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if session.update(&[view()], 0.02).unwrap().load_failed {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Missing root did not report failure");
}
#[test]
fn missing_child_does_not_mark_a_renderable_tileset_unavailable() {
    let server = Server::new();
    let mut session = Session::open_url(&format!("{}/missing-child.json", server.url)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut visible_frames = 0;
    while Instant::now() < deadline {
        let frame = session.update(&[view()], 0.02).unwrap();
        assert!(
            !frame.load_failed,
            "A failed child must not poison the tileset status"
        );
        if !frame.visible.is_empty() {
            visible_frames += 1;
        }
        if visible_frames >= 60 && server.missing.load(Ordering::Relaxed) > 0 {
            assert!(
                !frame.selection_complete,
                "A missing child must block capture readiness"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Valid parent was not rendered despite the missing child");
}

#[test]
fn root_http_404_reports_a_tileset_failure() {
    let server = Server::new();
    let mut session = Session::open_url(&format!("{}/missing-root.json", server.url)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if session.update(&[view()], 0.02).unwrap().load_failed {
            assert!(server.missing.load(Ordering::Relaxed) > 0);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("Root HTTP 404 did not report failure");
}
