use crate::{Error, Result};
use glam::DVec3;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

pub(crate) const A: f64 = 6_378_137.;

/// An oblate reference ellipsoid, in meters. Heights are ellipsoidal, not datum heights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipsoid {
    pub(crate) semi_major: f64,
    pub(crate) semi_minor: f64,
}
impl Default for Ellipsoid {
    fn default() -> Self {
        Self::WGS84
    }
}
impl Ellipsoid {
    pub const WGS84: Self = Self {
        semi_major: A,
        semi_minor: 6_356_752.314_245_179,
    };
    /// IAU 2000 Mars reference axes. Source datasets may instead use a sphere.
    pub const MARS: Self = Self {
        semi_major: 3_396_190.,
        semi_minor: 3_376_200.,
    };
    /// Construct an oblate ellipsoid (or sphere), with finite positive axes in meters.
    pub fn new(semi_major: f64, semi_minor: f64) -> Result<Self> {
        if !semi_major.is_finite()
            || !semi_minor.is_finite()
            || semi_minor <= 0.
            || semi_major < semi_minor
            || semi_major > 1e9
            || !(0.0..1.0).contains(&(1. - (semi_minor / semi_major).powi(2)))
        {
            return Err(Error("Invalid ellipsoid axes"));
        }
        Ok(Self {
            semi_major,
            semi_minor,
        })
    }
    pub fn radii(self) -> [f64; 3] {
        [self.semi_major, self.semi_major, self.semi_minor]
    }
    /// Convert longitude/geodetic latitude in radians and ellipsoidal height in
    /// meters to body-fixed Cartesian meters. This does not convert a vertical datum.
    pub fn cartesian(self, lon: f64, lat: f64, height: f64) -> DVec3 {
        let e2 = 1. - (self.semi_minor / self.semi_major).powi(2);
        let n = self.semi_major / (1. - e2 * lat.sin().powi(2)).sqrt();
        DVec3::new(
            (n + height) * lat.cos() * lon.cos(),
            (n + height) * lat.cos() * lon.sin(),
            (n * (1. - e2) + height) * lat.sin(),
        )
    }
    pub(crate) fn region_sphere(self, r: [f64; 6]) -> Result<(DVec3, f64)> {
        region_sphere_on(r, self)
    }
}

#[cfg(test)]
pub(crate) fn ecef(lon: f64, lat: f64, height: f64) -> DVec3 {
    Ellipsoid::WGS84.cartesian(lon, lat, height)
}

// Regions are radians/meters on the selected ellipsoid, independent of tile transforms.
// A curvature bound encloses the whole curved patch, not just its corners.
#[cfg(test)]
pub(crate) fn region_sphere(r: [f64; 6]) -> Result<(DVec3, f64)> {
    Ellipsoid::WGS84.region_sphere(r)
}
fn region_sphere_on(r: [f64; 6], ellipsoid: Ellipsoid) -> Result<(DVec3, f64)> {
    let [west, south, east, north, low, high] = r;
    if r.iter().any(|v| !v.is_finite())
        || west.abs() > PI
        || east.abs() > PI
        || south < -FRAC_PI_2
        || north > FRAC_PI_2
        || south > north
        || low > high
        || low <= -ellipsoid.semi_minor
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
        return Ok((DVec3::ZERO, ellipsoid.semi_major + high.max(0.)));
    }
    let center = ellipsoid.cartesian(
        west + width * 0.5,
        (south + north) * 0.5,
        (low + high) * 0.5,
    );
    let curvature_bound =
        ellipsoid.semi_major / (ellipsoid.semi_minor / ellipsoid.semi_major).powi(2);
    let radius =
        (curvature_bound + low.abs().max(high.abs())) * angular_radius + (high - low) * 0.5;
    Ok((center, radius))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mars_axes_sphere_and_invalid_ellipsoids() {
        let mars = Ellipsoid::MARS;
        assert!((mars.cartesian(0., 0., 0.).x - 3_396_190.).abs() < 1e-8);
        assert!((mars.cartesian(0., FRAC_PI_2, 0.).z - 3_376_200.).abs() < 1e-8);
        let sphere = Ellipsoid::new(3_396_190., 3_396_190.).unwrap();
        assert!((sphere.cartesian(0.2, 0.7, 100.).length() - 3_396_290.).abs() < 1e-8);
        for (a, b) in [
            (0., 0.),
            (1., 2.),
            (f64::NAN, 1.),
            (1., f64::INFINITY),
            (1., 1e-100),
        ] {
            assert!(Ellipsoid::new(a, b).is_err());
        }
        let region = [1.2, 0.2, 1.21, 0.21, -100., 500.];
        let (center, radius) = mars.region_sphere(region).unwrap();
        for lon in [region[0], region[2]] {
            for lat in [region[1], region[3]] {
                for height in [region[4], region[5]] {
                    assert!(mars.cartesian(lon, lat, height).distance(center) <= radius);
                }
            }
        }
    }
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
