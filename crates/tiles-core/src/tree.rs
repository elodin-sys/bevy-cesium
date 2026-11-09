use crate::{Error, Result, View};
use glam::{DMat4, DVec3};
use serde::Deserialize;
use url::Url;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonTile {
    bounding_volume: Volume,
    geometric_error: f64,
    transform: Option<[f64; 16]>,
    refine: Option<String>,
    content: Option<Content>,
    #[serde(default)]
    children: Vec<JsonTile>,
    implicit_tiling: Option<serde_json::Value>,
    contents: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct Content {
    #[serde(alias = "url")]
    uri: String,
}
#[derive(Deserialize)]
struct Volume {
    #[serde(rename = "box")]
    axes: Option<[f64; 12]>,
    sphere: Option<[f64; 4]>,
    region: Option<[f64; 6]>,
}
#[derive(Debug)]
pub struct Node {
    pub terrain: Option<crate::terrain::TileAddress>,
    pub depth: usize,
    pub transform: DMat4,
    pub center: DVec3,
    pub radius: f64,
    pub error: f64,
    pub children: Vec<usize>,
    pub url: Option<Url>,
    pub add: bool,
}
pub fn parse_tileset(bytes: &[u8], base: &Url) -> Result<Vec<Node>> {
    parse_tileset_at(bytes, base, DMat4::IDENTITY, false, 0)
}
pub(crate) fn parse_tileset_at(
    bytes: &[u8],
    base: &Url,
    parent: DMat4,
    add: bool,
    depth: usize,
) -> Result<Vec<Node>> {
    #[derive(Deserialize)]
    struct Tileset {
        root: JsonTile,
    }
    let data: Tileset =
        serde_json::from_slice(bytes).map_err(|_| Error("Invalid or unsupported tileset JSON"))?;
    let mut nodes = Vec::new();
    append(data.root, parent, add, base, &mut nodes, depth)?;
    Ok(nodes)
}
fn append(
    tile: JsonTile,
    parent: DMat4,
    inherited_add: bool,
    base: &Url,
    nodes: &mut Vec<Node>,
    depth: usize,
) -> Result<usize> {
    if depth > 48
        || nodes.len() >= 100_000
        || tile.implicit_tiling.is_some()
        || tile.contents.is_some()
    {
        return Err(Error("Unsupported tileset hierarchy"));
    }
    let transform = parent
        * tile
            .transform
            .map(|v| DMat4::from_cols_array(&v))
            .unwrap_or(DMat4::IDENTITY);
    if !transform.is_finite() || transform.determinant().abs() < 1e-20 {
        return Err(Error("Invalid tile transform"));
    }
    let (center, radius) = if let Some(b) = tile.bounding_volume.axes {
        let axis = |i| transform.transform_vector3(DVec3::new(b[i], b[i + 1], b[i + 2]));
        (
            transform.transform_point3(DVec3::new(b[0], b[1], b[2])),
            [-1., 1.]
                .into_iter()
                .flat_map(|x| {
                    [-1., 1.].into_iter().flat_map(move |y| {
                        [-1., 1.]
                            .into_iter()
                            .map(move |z| (axis(3) * x + axis(6) * y + axis(9) * z).length())
                    })
                })
                .fold(0., f64::max),
        )
    } else if let Some(s) = tile.bounding_volume.sphere {
        (
            transform.transform_point3(DVec3::new(s[0], s[1], s[2])),
            s[3] * max_scale(transform),
        )
    } else if let Some(region) = tile.bounding_volume.region {
        crate::geography::region_sphere(region)?
    } else {
        return Err(Error("Unsupported bounding volume"));
    };
    if !center.is_finite()
        || !radius.is_finite()
        || radius < 0.
        || !tile.geometric_error.is_finite()
        || tile.geometric_error < 0.
    {
        return Err(Error("Invalid tile bounds/error"));
    }
    let add = match tile.refine.as_deref() {
        Some("ADD") => true,
        Some("REPLACE") => false,
        None => inherited_add,
        _ => return Err(Error("Unknown refinement")),
    };
    let url = tile
        .content
        .map(|c| crate::resolve(base, &c.uri))
        .transpose()?;
    let id = nodes.len();
    nodes.push(Node {
        terrain: None,
        depth,
        transform,
        center,
        radius,
        error: tile.geometric_error * max_scale(transform),
        children: vec![],
        url,
        add,
    });
    for child in tile.children {
        let c = append(child, transform, add, base, nodes, depth + 1)?;
        nodes[id].children.push(c);
    }
    Ok(id)
}
// Frobenius bound supports nonuniform scale and shear conservatively.
fn max_scale(m: DMat4) -> f64 {
    let a = m.x_axis.truncate();
    let b = m.y_axis.truncate();
    let c = m.z_axis.truncate();
    let tolerance = 1e-12 * (a.length_squared() + b.length_squared() + c.length_squared());
    if a.dot(b).abs() < tolerance && a.dot(c).abs() < tolerance && b.dot(c).abs() < tolerance {
        return a.length().max(b.length()).max(c.length());
    }
    (m.x_axis.truncate().length_squared()
        + m.y_axis.truncate().length_squared()
        + m.z_axis.truncate().length_squared())
    .sqrt()
}
impl Node {
    pub fn screen_error(&self, v: &View) -> Option<f64> {
        let p = self.center - DVec3::from_array(v.position);
        let forward = DVec3::from_array(v.direction).normalize();
        let right = forward.cross(DVec3::from_array(v.up)).normalize();
        let up = right.cross(forward);
        let z = p.dot(forward);
        let hx = v.horizontal_fov * 0.5;
        let hy = v.vertical_fov * 0.5;
        if z < -self.radius
            || p.dot(right).abs() * hx.cos() - z * hx.sin() > self.radius
            || p.dot(up).abs() * hy.cos() - z * hy.sin() > self.radius
        {
            return None;
        }
        let distance = (p.length() - self.radius).max(0.01);
        Some(self.error * v.height / (2. * hy.tan() * distance))
    }
}
