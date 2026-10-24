use cesium_native_bridge::{CartographicPosition, Session};
use std::time::Duration;

#[test]
fn heights_are_sampled_without_a_render_camera() {
    let url = format!(
        "file://{}/tests/data/height.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut session = Session::open_url(&url).unwrap();
    // The triangle lies on the tangent plane x=6378137, in y>0,z>0.
    let positions = [CartographicPosition {
        longitude_radians: 2.0 / 6_378_137.0,
        latitude_radians: 2.0 / 6_335_439.0,
    }];
    let heights = session
        .sample_heights(&positions, Duration::from_secs(10))
        .unwrap();
    assert_eq!(heights.len(), 1);
    assert!(heights[0].abs() < 0.01, "height={}", heights[0]);
}

#[test]
fn missing_or_invalid_heights_are_errors_not_zero_elevations() {
    let url = format!(
        "file://{}/tests/data/height.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut session = Session::open_url(&url).unwrap();
    for position in [
        CartographicPosition {
            longitude_radians: 1.0,
            latitude_radians: 1.0,
        },
        CartographicPosition {
            longitude_radians: f64::NAN,
            latitude_radians: 0.0,
        },
    ] {
        assert!(
            session
                .sample_heights(&[position], Duration::from_secs(10))
                .is_err()
        );
    }
    assert!(
        session
            .sample_heights(&[], Duration::from_secs(10))
            .is_err()
    );
}

#[test]
#[ignore = "requires CESIUM_ION_TOKEN and network access"]
fn ion_height_grid() {
    let token = std::env::var("CESIUM_ION_TOKEN").unwrap();
    let side: usize = std::env::var("HEIGHT_GRID_SIDE")
        .unwrap_or("3".into())
        .parse()
        .unwrap();
    let positions: Vec<_> = (0..side)
        .flat_map(|row| {
            (0..side).map(move |column| CartographicPosition {
                longitude_radians: (-117.824 + (column as f64 - (side / 2) as f64) * 0.0005)
                    .to_radians(),
                latitude_radians: (35.431 + (row as f64 - (side / 2) as f64) * 0.0005).to_radians(),
            })
        })
        .collect();
    let started = std::time::Instant::now();
    let mut session = Session::open_terrain_ion(1, &token).unwrap();
    let heights = session
        .sample_heights(&positions, Duration::from_secs(600))
        .unwrap();
    eprintln!(
        "{} heights in {:?}; center={} m ellipsoid",
        heights.len(),
        started.elapsed(),
        heights[heights.len() / 2]
    );
    assert!(
        heights
            .iter()
            .all(|h| h.is_finite() && *h > -500.0 && *h < 9000.0)
    );
}
