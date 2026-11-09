//! Bounded quantized-mesh terrain. Static availability, EPSG:4326/TMS.
use crate::{
    Error, Node, Primitive, Result,
    geography::{A, ecef, region_sphere},
};
use glam::{DMat4, DVec3, Vec3};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashMap},
    f64::consts::{FRAC_PI_2, PI},
};
use url::Url;

#[derive(Clone, Copy, Debug)]
pub struct TileAddress {
    pub level: u32,
    pub x: u32,
    pub y: u32,
}
#[derive(Clone)]
pub struct Imagery {
    pub url_template: String,
    pub credit: String,
}
impl Imagery {
    pub(crate) fn url(&self, address: TileAddress) -> Result<Url> {
        if !["{z}", "{x}", "{y}"]
            .iter()
            .all(|s| self.url_template.contains(s))
            || self.credit.trim().is_empty()
        {
            return Err(Error(
                "Imagery requires a geographic TMS template and attribution",
            ));
        }
        let text = self
            .url_template
            .replace("{z}", &address.level.to_string())
            .replace("{x}", &address.x.to_string())
            .replace("{y}", &address.y.to_string());
        if text.contains(['{', '}']) {
            return Err(Error("Unknown imagery URL variable"));
        }
        let url = Url::parse(&text).map_err(|_| Error("Invalid imagery URL"))?;
        if !matches!(url.scheme(), "https" | "http" | "file") {
            return Err(Error("Unsupported imagery URL"));
        }
        Ok(url)
    }
}
pub(crate) fn apply_image(p: &mut Primitive, bytes: &[u8]) -> Result<()> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Error("Unknown imagery format"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| Error("Invalid or oversized imagery"))?
        .into_rgba8();
    p.width = image.width();
    p.height = image.height();
    p.pixels = image.into_raw();
    Ok(())
}
impl TileAddress {
    pub(crate) fn region(self) -> [f64; 6] {
        let step = PI / (1u64 << self.level) as f64;
        [
            -PI + self.x as f64 * step,
            -FRAC_PI_2 + self.y as f64 * step,
            -PI + (self.x + 1) as f64 * step,
            -FRAC_PI_2 + (self.y + 1) as f64 * step,
            -12_000.,
            12_000.,
        ]
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layer {
    format: String,
    tiles: Vec<String>,
    maxzoom: u32,
    version: Option<String>,
    projection: Option<String>,
    scheme: Option<String>,
    available: Option<Vec<Vec<Rectangle>>>,
    metadata_availability: Option<u32>,
    parent_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rectangle {
    start_x: u32,
    start_y: u32,
    end_x: u32,
    end_y: u32,
}

pub(crate) fn layer(bytes: &[u8], base: &Url) -> Result<Vec<Node>> {
    let l: Layer = serde_json::from_slice(bytes).map_err(|_| Error("Invalid terrain layer"))?;
    if l.format != "quantized-mesh-1.0"
        || l.projection.as_deref().is_some_and(|p| p != "EPSG:4326")
        || l.scheme.as_deref().is_some_and(|s| s != "tms")
        || l.metadata_availability.is_some()
        || l.parent_url.is_some()
        || l.maxzoom > 24
        || l.tiles.is_empty()
    {
        return Err(Error(
            "Unsupported terrain projection, hierarchy or availability",
        ));
    }
    let mut available = BTreeSet::new();
    if let Some(levels) = l.available {
        if levels.len() > 25 {
            return Err(Error("Terrain level limit exceeded"));
        }
        for (z, ranges) in levels.into_iter().enumerate() {
            if z as u32 > l.maxzoom {
                return Err(Error("Terrain availability exceeds maxzoom"));
            }
            for r in ranges {
                if r.start_x > r.end_x
                    || r.start_y > r.end_y
                    || r.end_x >= (2u32 << z)
                    || r.end_y >= (1u32 << z)
                    || (r.end_x - r.start_x + 1) as u64 * (r.end_y - r.start_y + 1) as u64 > 60_000
                {
                    return Err(Error("Invalid terrain availability range"));
                }
                for x in r.start_x..=r.end_x {
                    for y in r.start_y..=r.end_y {
                        available.insert((z as u32, x, y));
                        if available.len() > 60_000 {
                            return Err(Error("Terrain hierarchy limit exceeded"));
                        }
                    }
                }
            }
        }
    } else {
        if l.maxzoom > 6 {
            return Err(Error("Large terrain requires explicit static availability"));
        }
        for z in 0..=l.maxzoom {
            for x in 0..(2 << z) {
                for y in 0..(1 << z) {
                    available.insert((z, x, y));
                }
            }
        }
    }
    let mut keys = available.clone();
    for &(z, x, y) in &available {
        for parent in 0..z {
            keys.insert((parent, x >> (z - parent), y >> (z - parent)));
        }
    }
    if keys.len() > 99_999 {
        return Err(Error("Terrain hierarchy limit exceeded"));
    }
    let mut nodes = vec![Node {
        depth: 0,
        transform: DMat4::IDENTITY,
        center: DVec3::ZERO,
        radius: A + 12_000.,
        error: A,
        children: vec![],
        url: None,
        add: false,
        terrain: None,
    }];
    let mut ids = HashMap::new();
    for (z, x, y) in keys {
        let address = TileAddress { level: z, x, y };
        let (center, radius) = region_sphere(address.region())?;
        let uri = l.tiles[0]
            .replace("{z}", &z.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string())
            .replace("{version}", l.version.as_deref().unwrap_or("1.0.0"));
        if uri.contains(['{', '}']) {
            return Err(Error("Unknown terrain URL variable"));
        }
        let url = if available.contains(&(z, x, y)) {
            Some(crate::resolve(base, &uri)?)
        } else {
            None
        };
        let id = nodes.len();
        let parent = if z == 0 {
            0
        } else {
            *ids.get(&(z - 1, x / 2, y / 2))
                .ok_or(Error("Missing terrain ancestor"))?
        };
        nodes[parent].children.push(id);
        nodes.push(Node {
            depth: z as usize + 1,
            transform: DMat4::IDENTITY,
            center,
            radius,
            error: A * PI / (65. * (1u64 << z) as f64),
            children: vec![],
            url,
            add: false,
            terrain: Some(address),
        });
        ids.insert((z, x, y), id);
    }
    // Retain parent coverage when fewer than four children exist. Do not
    // replace a whole patch by only a partial set of children.
    for n in &mut nodes {
        if n.url.is_some() && !n.children.is_empty() && n.children.len() != 4 {
            n.children.clear();
        }
    }
    Ok(nodes)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self
            .at
            .checked_add(N)
            .ok_or(Error("Terrain length overflow"))?;
        let value = self
            .bytes
            .get(self.at..end)
            .ok_or(Error("Truncated terrain"))?
            .try_into()
            .map_err(|_| Error("Truncated terrain"))?;
        self.at = end;
        Ok(value)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take()?))
    }
    fn index(&mut self, wide: bool) -> Result<u32> {
        if wide {
            self.u32()
        } else {
            self.u16().map(u32::from)
        }
    }
}

pub(crate) fn decode(bytes: &[u8], address: TileAddress) -> Result<Primitive> {
    if address.level > 24 || address.x >= (2 << address.level) || address.y >= (1 << address.level)
    {
        return Err(Error("Invalid terrain tile address"));
    }
    let mut r = Reader { bytes, at: 0 };
    let center = DVec3::new(r.f64()?, r.f64()?, r.f64()?);
    let low = r.f32()? as f64;
    let high = r.f32()? as f64;
    if !center.is_finite()
        || center.length() > 1e9
        || !low.is_finite()
        || !high.is_finite()
        || low > high
        || low < -12_000.
        || high > 12_000.
    {
        return Err(Error("Invalid terrain header"));
    }
    for _ in 0..7 {
        if !r.f64()?.is_finite() {
            return Err(Error("Invalid terrain header"));
        }
    }
    let count = r.u32()? as usize;
    if !(3..=1_000_000).contains(&count) {
        return Err(Error("Terrain vertex limit exceeded"));
    }
    let mut values = Vec::with_capacity(3 * count);
    for _ in 0..3 {
        let mut value = 0i32;
        for _ in 0..count {
            let code = r.u16()? as i32;
            value += (code >> 1) ^ (-(code & 1));
            if !(0..=32767).contains(&value) {
                return Err(Error("Invalid terrain delta"));
            }
            values.push(value as f64 / 32767.);
        }
    }
    let wide = count > 65536;
    let alignment = if wide { 4 } else { 2 };
    r.at = r.at.next_multiple_of(alignment);
    let triangles = r.u32()? as usize;
    if triangles == 0 || triangles > 2_000_000 {
        return Err(Error("Terrain triangle limit exceeded"));
    }
    let mut indices = Vec::with_capacity(3 * triangles);
    let mut highest = 0u32;
    for _ in 0..3 * triangles {
        let code = r.index(wide)?;
        let index = highest
            .checked_sub(code)
            .filter(|i| *i < count as u32)
            .ok_or(Error("Invalid terrain index"))?;
        indices.push(index);
        if code == 0 {
            highest += 1;
        }
    }
    let mut edges = Vec::new();
    for _ in 0..4 {
        let size = r.u32()? as usize;
        if size > count {
            return Err(Error("Terrain edge limit exceeded"));
        }
        let mut edge = Vec::with_capacity(size);
        for _ in 0..size {
            let i = r.index(wide)?;
            if i >= count as u32 {
                return Err(Error("Invalid terrain edge"));
            }
            edge.push(i);
        }
        edges.push(edge);
    }
    // Extensions are length-delimited; normals are derived from the mesh.
    while r.at < bytes.len() {
        let _id = r.take::<1>()?;
        let length = r.u32()? as usize;
        r.at =
            r.at.checked_add(length)
                .filter(|n| *n <= bytes.len())
                .ok_or(Error("Truncated terrain extension"))?;
    }
    let region = address.region();
    let mut positions = Vec::<Vec3>::with_capacity(count);
    let mut uvs = Vec::with_capacity(count * 2);
    for i in 0..count {
        let u = values[i];
        let v = values[count + i];
        let h = values[2 * count + i];
        positions.push(
            (ecef(
                region[0] + (region[2] - region[0]) * u,
                region[1] + (region[3] - region[1]) * v,
                low + (high - low) * h,
            ) - center)
                .as_vec3(),
        );
        uvs.extend([u as f32, 1. - v as f32]);
    }
    let mut normals = vec![Vec3::ZERO; count];
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let n = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        for i in [a, b, c] {
            normals[i] += n;
        }
    }
    for (i, n) in normals.iter_mut().enumerate() {
        *n = n
            .try_normalize()
            .unwrap_or_else(|| (center + positions[i].as_dvec3()).normalize().as_vec3());
    }
    let skirt = (A * PI / (65. * (1u64 << address.level) as f64)).clamp(1., 1000.) as f32;
    for edge in edges {
        let start = positions.len() as u32;
        for &i in &edge {
            let i = i as usize;
            let radial = (center + positions[i].as_dvec3()).normalize().as_vec3();
            positions.push(positions[i] - radial * skirt);
            normals.push(normals[i]);
            uvs.extend([uvs[2 * i], uvs[2 * i + 1]]);
        }
        for (j, pair) in edge.windows(2).enumerate() {
            let s = start + j as u32;
            indices.extend([pair[0], s, pair[1], pair[1], s, s + 1]);
        }
    }
    Ok(Primitive {
        transform: DMat4::from_translation(center).to_cols_array(),
        positions: positions.iter().flat_map(|v| v.to_array()).collect(),
        normals: normals.iter().flat_map(|v| v.to_array()).collect(),
        uvs,
        indices,
        base_color: [1.; 4],
        pixels: vec![],
        width: 0,
        height: 0,
        double_sided: true,
        unlit: false,
        metallic: 0.,
        roughness: 1.,
        emissive: [0.; 3],
        alpha_mode: 0,
        alpha_cutoff: 0.5,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    #[ignore = "requires CESIUM_NATIVE_TEST_DATA from the pinned Native source checkout"]
    fn independent_cesium_native_terrain_fixtures_decode() {
        let root = std::path::PathBuf::from(
            std::env::var("CESIUM_NATIVE_TEST_DATA").expect("set CESIUM_NATIVE_TEST_DATA"),
        );
        for (file, address) in [
            (
                "9_759_335/9_759_335.terrain",
                TileAddress {
                    level: 9,
                    x: 759,
                    y: 335,
                },
            ),
            (
                "tile.32bitIndices.terrain",
                TileAddress {
                    level: 0,
                    x: 0,
                    y: 0,
                },
            ),
        ] {
            let bytes = std::fs::read(root.join(file)).unwrap();
            let p = decode(&bytes, address).unwrap();
            assert!(!p.indices.is_empty());
            assert!(p.positions.iter().all(|v| v.is_finite()));
            assert!(
                p.indices
                    .iter()
                    .all(|i| (*i as usize) < p.positions.len() / 3)
            );
        }
    }
    pub(crate) fn fixture(address: TileAddress) -> Vec<u8> {
        let r = address.region();
        let center = ecef((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5, 0.);
        let mut bytes = Vec::new();
        for x in center.to_array() {
            bytes.extend(x.to_le_bytes());
        }
        for x in [100f32, 200.] {
            bytes.extend(x.to_le_bytes());
        }
        for _ in 0..7 {
            bytes.extend(0f64.to_le_bytes());
        }
        bytes.extend(4u32.to_le_bytes());
        for values in [
            [0, 32767, 0, 32767],
            [0, 0, 32767, 32767],
            [0, 0, 32767, 32767],
        ] {
            let mut previous = 0i32;
            for value in values {
                let delta = value - previous;
                previous = value;
                bytes.extend((((delta << 1) ^ (delta >> 31)) as u16).to_le_bytes());
            }
        }
        bytes.extend(2u32.to_le_bytes());
        for code in [0u16, 0, 0, 2, 0, 2] {
            bytes.extend(code.to_le_bytes());
        }
        for edge in [[0u16, 2], [1, 0], [3, 1], [2, 3]] {
            bytes.extend(2u32.to_le_bytes());
            for i in edge {
                bytes.extend(i.to_le_bytes());
            }
        }
        bytes
    }
    #[test]
    fn quantized_mesh_restores_ecef_height_winding_uvs_and_skirts() {
        let a = TileAddress {
            level: 10,
            x: 1024,
            y: 512,
        };
        let p = decode(&fixture(a), a).unwrap();
        let transform = DMat4::from_cols_array(&p.transform);
        let r = a.region();
        let expected = ecef(r[0], r[1], 100.);
        let actual = transform.transform_point3(DVec3::new(
            p.positions[0] as f64,
            p.positions[1] as f64,
            p.positions[2] as f64,
        ));
        assert!((actual - expected).length() < 0.002);
        assert_eq!(&p.uvs[..2], &[0., 1.]);
        assert_eq!(&p.indices[..6], &[0, 1, 2, 1, 3, 2]);
        let normal = DVec3::new(
            p.normals[0] as f64,
            p.normals[1] as f64,
            p.normals[2] as f64,
        );
        assert!(normal.dot(expected.normalize()) > 0.9);
        assert_eq!(p.positions.len() / 3, 12);
        assert_eq!(p.indices.len(), 30);
    }
    #[test]
    fn malformed_terrain_fails_without_allocating_unbounded_buffers() {
        let a = TileAddress {
            level: 10,
            x: 1024,
            y: 512,
        };
        let bytes = fixture(a);
        for end in [0, 8, 87, 90, 100, bytes.len() - 1] {
            assert!(decode(&bytes[..end], a).is_err());
        }
        let mut bad = bytes.clone();
        bad[88..92].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&bad, a).is_err());
        let mut bad = bytes;
        bad[92..94].copy_from_slice(&1u16.to_le_bytes());
        assert!(decode(&bad, a).is_err());
    }
    #[test]
    fn layer_coordinates_tms_and_static_availability_are_bounded() {
        let base = Url::parse("https://example.com/terrain/layer.json").unwrap();
        let nodes=layer(br#"{"format":"quantized-mesh-1.0","maxzoom":0,"tiles":["{z}/{x}/{y}.terrain?v={version}"],"version":"1"}"#,&base).unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].children, [1, 2]);
        assert_eq!(
            nodes[2].url.as_ref().unwrap().as_str(),
            "https://example.com/terrain/0/1/0.terrain?v=1"
        );
        assert_eq!(nodes[2].terrain.unwrap().region()[0], 0.);
        for field in [
            r#""projection":"EPSG:3857""#,
            r#""metadataAvailability":10"#,
            r#""parentUrl":"../layer.json""#,
        ] {
            let json =
                format!(r#"{{"format":"quantized-mesh-1.0","maxzoom":0,"tiles":["a"],{field}}}"#);
            assert!(layer(json.as_bytes(), &base).is_err());
        }
    }
}
