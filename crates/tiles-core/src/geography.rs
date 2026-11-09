use crate::{Error, Result};
use glam::DVec3;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

pub(crate) const A: f64 = 6_378_137.;
const E2: f64 = 0.006_694_379_990_141_316_5;

pub(crate) fn ecef(lon: f64, lat: f64, height: f64) -> DVec3 {
    let n = A / (1. - E2 * lat.sin().powi(2)).sqrt();
    DVec3::new(
        (n + height) * lat.cos() * lon.cos(),
        (n + height) * lat.cos() * lon.sin(),
        (n * (1. - E2) + height) * lat.sin(),
    )
}

// Regions are WGS84 radians/meters, independent of the tile's transform.
// A curvature bound encloses the whole curved patch, not just its corners.
pub(crate) fn region_sphere(r: [f64; 6]) -> Result<(DVec3, f64)> {
    let [west, south, east, north, low, high] = r;
    if r.iter().any(|v| !v.is_finite())
        || west.abs() > PI
        || east.abs() > PI
        || south < -FRAC_PI_2
        || north > FRAC_PI_2
        || south > north
        || low > high
        || low < -6_000_000.
        || high > 1e9
    {
        return Err(Error("Invalid geographic region"));
    }
    let width = if east < west {
        east + TAU - west
    } else {
        east - west
    };
    let angular_radius = (width + north - south) * 0.5;
    if angular_radius >= PI {
        return Ok((DVec3::ZERO, A + high.max(0.)));
    }
    let center = ecef(
        west + width * 0.5,
        (south + north) * 0.5,
        (low + high) * 0.5,
    );
    let radius = (A / (1. - E2) + low.abs().max(high.abs())) * angular_radius + (high - low) * 0.5;
    Ok((center, radius))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wgs84_axes_and_region_enclosure() {
        assert!((ecef(0., 0., 0.) - DVec3::X * A).length() < 1e-8);
        assert!((ecef(0., FRAC_PI_2, 0.).z - 6_356_752.314_245).abs() < 1e-5);
        for r in [
            [3., -0.2, -3., 0.3, -50., 5000.],
            [-PI, -FRAC_PI_2, PI, FRAC_PI_2, 0., 9000.],
            [0.1, 1.4, 0.4, FRAC_PI_2, 0., 200.],
        ] {
            let (c, radius) = region_sphere(r).unwrap();
            let east = if r[2] < r[0] { r[2] + TAU } else { r[2] };
            for i in 0..=20 {
                for j in 0..=20 {
                    for h in [r[4], r[5]] {
                        let p = ecef(
                            r[0] + (east - r[0]) * i as f64 / 20.,
                            r[1] + (r[3] - r[1]) * j as f64 / 20.,
                            h,
                        );
                        assert!((p - c).length() <= radius + 1e-6);
                    }
                }
            }
        }
    }
    #[test]
    fn invalid_regions_fail() {
        assert!(region_sphere([0., 1., 0., 0., 0., 0.]).is_err());
        assert!(region_sphere([f64::NAN, 0., 0., 0., 0., 0.]).is_err());
    }
}
