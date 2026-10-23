//! Opt-in CPU timing for the RC-jet terrain selector; no rendering or network timing.
//! CESIUM_ION_TOKEN must be set. Pass the retention margin in meters as argument 1.
use cesium_native_bridge::{Frame, Session, View};
use std::time::{Duration, Instant};

fn warm(session: &mut Session, views: &[View]) -> Frame {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut stable = 0;
    let mut previous = Vec::new();
    loop {
        let frame = session.update(views, 1.0 / 60.0).unwrap();
        assert!(!frame.load_failed, "terrain loading failed");
        if frame.selection_complete && frame.visible == previous {
            stable += 1;
        } else {
            stable = 0;
        }
        previous = frame.visible.clone();
        if stable >= 8 {
            return frame;
        }
        assert!(Instant::now() < deadline, "terrain warm-up timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn measure(session: &mut Session, views: &[View], margin: f64, phase: &str) {
    let frame = warm(session, views);
    let mut times = Vec::new();
    for _ in 0..120 {
        let start = Instant::now();
        let next = session.update(views, 1.0 / 60.0).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(next.selection_complete);
        assert_eq!(next.visible, frame.visible);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "margin={margin} phase={phase} visible={} retained={} update_p50_ms={:.3} update_p95_ms={:.3}",
        frame.visible.len(),
        frame.retained_tiles,
        times[60],
        times[114]
    );
}

fn main() {
    let margin: f64 = std::env::args()
        .nth(1)
        .expect("retention margin")
        .parse()
        .unwrap();
    let token = std::env::var("CESIUM_ION_TOKEN").expect("CESIUM_ION_TOKEN");
    let mut session = Session::open_ion(1, &token).unwrap();
    session.configure(16.0, 256 * 1024 * 1024).unwrap();
    session.configure_retention(margin).unwrap();
    let near = View {
        position: [-2430012.810493948, -4607169.833479509, 3670181.2260014],
        direction: [0.13110570425775708, 0.6203194220676107, 0.7733143661647461],
        up: [-0.38051087534012884, -0.721427565569737, 0.5785790709868548],
        width: 1280.0,
        height: 720.0,
        horizontal_fov: 1.4191207759378222,
        vertical_fov: 0.9,
    };
    measure(&mut session, &[near], margin, "near");
    let mut far = near;
    for axis in 0..3 {
        far.position[axis] -= 1000.0 * near.direction[axis];
    }
    measure(&mut session, &[far], margin, "retreat_1000m");
    let mut turn = far;
    turn.direction = [0.898, -0.440, 0.0];
    measure(&mut session, &[turn], margin, "turn");
    measure(&mut session, &[near], margin, "return");
}
