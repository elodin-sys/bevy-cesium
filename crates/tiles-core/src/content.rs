use crate::{Error, Primitive, Result};
use base64::Engine;
use glam::{DMat4, DVec3, Vec3};
use url::Url;

pub(crate) fn external_tree(
    bytes: &[u8],
    tile: DMat4,
    add: bool,
    depth: usize,
    url: &Url,
) -> Result<Option<Vec<crate::Node>>> {
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| Error("Invalid content JSON"))?;
    if value.get("root").is_none() {
        return Ok(None);
    }
    crate::tree::parse_tileset_at(bytes, url, tile, add, depth + 1).map(Some)
}

pub(crate) fn credits(bytes: &[u8]) -> Result<Vec<String>> {
    let (gltf, _) = prepare(bytes, DMat4::IDENTITY)?;
    Ok(gltf
        .document
        .as_json()
        .asset
        .copyright
        .as_deref()
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

fn word(b: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(
        b.get(at..at + 4)
            .ok_or(Error("Truncated b3dm header"))?
            .try_into()
            .map_err(|_| Error("Bad header"))?,
    ) as usize)
}
/// Decode a glTF/GLB or b3dm v1 tile, including Draco-compressed primitives.
#[cfg(not(target_arch = "wasm32"))]
pub fn decode(bytes: &[u8], tile: DMat4, url: &Url, agent: &ureq::Agent) -> Result<Vec<Primitive>> {
    decode_with(bytes, tile, url, &|u| crate::fetch(agent, u))
}

fn prepare(bytes: &[u8], tile: DMat4) -> Result<(gltf::Gltf, DMat4)> {
    let mut root = tile;
    let glb = if bytes.starts_with(b"b3dm") {
        if word(bytes, 4)? != 1 || word(bytes, 8)? != bytes.len() {
            return Err(Error("Invalid b3dm version or length"));
        }
        let feature_length = word(bytes, 12)?;
        let mut offset = 28usize;
        for at in [12, 16, 20, 24] {
            offset = offset
                .checked_add(word(bytes, at)?)
                .ok_or(Error("b3dm length overflow"))?;
        }
        let feature = bytes
            .get(28..28 + feature_length)
            .ok_or(Error("Truncated feature table"))?;
        if !feature.is_empty() {
            let f: serde_json::Value =
                serde_json::from_slice(feature).map_err(|_| Error("Invalid feature JSON"))?;
            if let Some(center) = f.get("RTC_CENTER") {
                let c: [f64; 3] = serde_json::from_value(center.clone())
                    .map_err(|_| Error("Unsupported RTC_CENTER"))?;
                root *= DMat4::from_translation(DVec3::from_array(c));
            }
        }
        bytes.get(offset..).ok_or(Error("Truncated b3dm payload"))?
    } else {
        bytes
    };
    let gltf = gltf::Gltf::from_slice_without_validation(glb)
        .map_err(|_| Error("Invalid glTF payload"))?;
    validate(&gltf.document)?;
    if gltf.accessors().any(|a| a.count() > 6_000_000) {
        return Err(Error("Accessor count limit exceeded"));
    }
    for extension in gltf.extensions_required() {
        if !matches!(
            extension,
            "KHR_draco_mesh_compression" | "KHR_materials_unlit"
        ) {
            return Err(Error("Unsupported required glTF extension"));
        }
    }
    if let Some(rtc) = gltf.extension_value("CESIUM_RTC") {
        let c: [f64; 3] = serde_json::from_value(rtc["center"].clone())
            .map_err(|_| Error("Invalid CESIUM_RTC"))?;
        root *= DMat4::from_translation(DVec3::from_array(c));
    }
    root *= DMat4::from_rotation_x(std::f64::consts::FRAC_PI_2);
    Ok((gltf, root))
}

pub(crate) fn decode_with(
    bytes: &[u8],
    tile: DMat4,
    url: &Url,
    load: &dyn Fn(&Url) -> Result<Vec<u8>>,
) -> Result<Vec<Primitive>> {
    let (gltf, root) = prepare(bytes, tile)?;
    let mut buffers = Vec::new();
    let mut total = 0usize;
    for b in gltf.buffers() {
        let data = match b.source() {
            gltf::buffer::Source::Bin => gltf.blob.clone().ok_or(Error("Missing GLB buffer"))?,
            gltf::buffer::Source::Uri(uri) => resource(uri, url, load)?,
        };
        total += data.len();
        if data.len() < b.length() || total > 64 * 1024 * 1024 {
            return Err(Error("Invalid buffer length or tile budget exceeded"));
        }
        buffers.push(data);
    }
    let scene = gltf
        .default_scene()
        .or_else(|| gltf.scenes().next())
        .ok_or(Error("No glTF scene"))?;
    let mut output = Vec::new();
    for node in scene.nodes() {
        visit(
            node,
            root,
            &gltf.document,
            &buffers,
            url,
            load,
            &mut output,
            0,
        )?;
    }
    if output.is_empty() {
        return Err(Error("No supported triangle primitives"));
    }
    Ok(output)
}
fn resource(uri: &str, base: &Url, load: &dyn Fn(&Url) -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    if uri.starts_with("data:") {
        let (header, encoded) = uri.split_once(',').ok_or(Error("Invalid data URI"))?;
        if !header.ends_with(";base64") || encoded.len() > 48 * 1024 * 1024 {
            return Err(Error("Unsupported data URI"));
        }
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| Error("Invalid base64"))
    } else {
        load(&crate::resolve(base, uri)?)
    }
}
fn slice<'a>(view: gltf::buffer::View<'_>, buffers: &'a [Vec<u8>]) -> Result<&'a [u8]> {
    buffers
        .get(view.buffer().index())
        .and_then(|b| b.get(view.offset()..view.offset().checked_add(view.length())?))
        .ok_or(Error("Invalid buffer view"))
}
#[allow(clippy::too_many_arguments)]
fn visit(
    node: gltf::Node<'_>,
    parent: DMat4,
    doc: &gltf::Document,
    buffers: &[Vec<u8>],
    url: &Url,
    load: &dyn Fn(&Url) -> Result<Vec<u8>>,
    out: &mut Vec<Primitive>,
    depth: usize,
) -> Result<()> {
    if depth > 48 || out.len() > 2048 {
        return Err(Error("glTF complexity limit exceeded"));
    }
    let m = node.transform().matrix().map(|c| c.map(f64::from));
    let transform = parent * DMat4::from_cols_array_2d(&m);
    if !transform.is_finite() {
        return Err(Error("Invalid node transform"));
    }
    if let Some(mesh) = node.mesh() {
        for p in mesh.primitives() {
            if p.mode() != gltf::mesh::Mode::Triangles {
                return Err(Error("Only triangle primitives are supported"));
            }
            let mat = p.material();
            let pbr = mat.pbr_metallic_roughness();
            let uv_set = pbr.base_color_texture().map_or(0, |t| t.tex_coord());
            let (positions, mut normals, mut uvs, indices) = if let Some(d) =
                p.extension_value("KHR_draco_mesh_compression")
            {
                let vi = d["bufferView"]
                    .as_u64()
                    .ok_or(Error("Missing Draco buffer view"))? as usize;
                let view = doc
                    .views()
                    .nth(vi)
                    .ok_or(Error("Draco view out of range"))?;
                decode_draco(d, slice(view, buffers)?, uv_set)?
            } else {
                let reader = p.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                let positions: Vec<f32> = reader
                    .read_positions()
                    .ok_or(Error("Missing positions"))?
                    .flatten()
                    .collect();
                let count = positions.len() / 3;
                let normals = reader
                    .read_normals()
                    .map(|n| n.flatten().collect())
                    .unwrap_or_default();
                let uvs = reader
                    .read_tex_coords(uv_set)
                    .map(|v| v.into_f32().flatten().collect())
                    .unwrap_or_default();
                let indices = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_else(|| (0..count as u32).collect());
                (positions, normals, uvs, indices)
            };
            let count = positions.len() / 3;
            if count == 0
                || count > 1_000_000
                || indices.len() % 3 != 0
                || indices.iter().any(|i| *i as usize >= count)
                || positions.iter().any(|v| !v.is_finite())
            {
                return Err(Error("Invalid triangle geometry"));
            }
            if normals.is_empty() {
                normals = vec![0.; positions.len()];
                for face in indices.as_chunks::<3>().0 {
                    let pos =
                        |i: u32| Vec3::from_slice(&positions[i as usize * 3..i as usize * 3 + 3]);
                    let n = (pos(face[1]) - pos(face[0])).cross(pos(face[2]) - pos(face[0]));
                    for i in face {
                        let s = &mut normals[*i as usize * 3..*i as usize * 3 + 3];
                        let sum = Vec3::from_slice(s) + n;
                        s.copy_from_slice(&sum.to_array());
                    }
                }
                for n in normals.as_chunks_mut::<3>().0 {
                    let value = Vec3::from_slice(n).normalize_or_zero();
                    n.copy_from_slice(&value.to_array());
                }
            }
            if uvs.is_empty() {
                uvs = vec![0.; count * 2];
            }
            if normals.len() != positions.len()
                || uvs.len() != count * 2
                || normals.iter().chain(&uvs).any(|v| !v.is_finite())
            {
                return Err(Error("Attribute size mismatch"));
            }
            let (pixels, width, height) = if let Some(texture) = pbr.base_color_texture() {
                let image = texture.texture().source();
                let encoded = match image.source() {
                    gltf::image::Source::View { view, .. } => slice(view, buffers)?.to_vec(),
                    gltf::image::Source::Uri { uri, .. } => resource(uri, url, load)?,
                };
                let mut reader = image::ImageReader::new(std::io::Cursor::new(encoded))
                    .with_guessed_format()
                    .map_err(|_| Error("Image format error"))?;
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(8192);
                limits.max_image_height = Some(8192);
                limits.max_alloc = Some(64 * 1024 * 1024);
                reader.limits(limits);
                let decoded = reader
                    .decode()
                    .map_err(|_| Error("Image decode failed"))?
                    .into_rgba8();
                let (w, h) = decoded.dimensions();
                (decoded.into_raw(), w, h)
            } else {
                (vec![], 0, 0)
            };
            out.push(Primitive {
                transform: transform.to_cols_array(),
                positions,
                normals,
                uvs,
                indices,
                pixels,
                width,
                height,
                base_color: pbr.base_color_factor(),
                double_sided: mat.double_sided(),
                unlit: mat.extension_value("KHR_materials_unlit").is_some(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                emissive: mat.emissive_factor(),
                alpha_mode: match mat.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => 0,
                    gltf::material::AlphaMode::Mask => 1,
                    gltf::material::AlphaMode::Blend => 2,
                },
                alpha_cutoff: mat.alpha_cutoff().unwrap_or(0.5),
            });
            let allocated: usize = out
                .iter()
                .map(|p| {
                    p.pixels.len()
                        + 4 * (p.positions.len() + p.normals.len() + p.uvs.len() + p.indices.len())
                })
                .sum();
            if allocated > 128 * 1024 * 1024 {
                return Err(Error("Decoded tile exceeds prototype budget"));
            }
        }
    }
    for child in node.children() {
        visit(child, transform, doc, buffers, url, load, out, depth + 1)?;
    }
    Ok(())
}
type Geometry = (Vec<f32>, Vec<f32>, Vec<f32>, Vec<u32>);
// gltf 1.4 does not know Draco. Keep its structural validation, allowing only
// the absent bufferViews for accessors supplied by a compressed primitive.
fn validate(doc: &gltf::Document) -> Result<()> {
    use gltf::json::validation::{Error as ValidationError, Validate};
    let root = doc.as_json();
    let mut draco_accessors = std::collections::HashSet::new();
    for mesh in &root.meshes {
        for p in &mesh.primitives {
            if p.extensions
                .as_ref()
                .is_some_and(|e| e.others.contains_key("KHR_draco_mesh_compression"))
            {
                for a in p.attributes.values() {
                    draco_accessors.insert(a.value());
                }
                if let Some(i) = p.indices {
                    draco_accessors.insert(i.value());
                }
            }
        }
    }
    let mut valid = true;
    root.validate(root, gltf::json::Path::new, &mut |path, error| {
        let path = path().to_string();
        let draco_view = draco_accessors
            .iter()
            .any(|i| path == format!("accessors[{i}].bufferView"));
        let supported_extension = path.starts_with("extensionsRequired[")
            && (path.contains("KHR_draco_mesh_compression")
                || path.contains("KHR_materials_unlit"));
        if !((error == ValidationError::Missing && draco_view)
            || (error == ValidationError::Unsupported && supported_extension))
        {
            valid = false;
        }
    });
    if valid {
        Ok(())
    } else {
        Err(Error("Invalid glTF structure"))
    }
}
fn decode_draco(d: &serde_json::Value, bytes: &[u8], uv_set: u32) -> Result<Geometry> {
    use draco_core::{DecodeLimits, DecoderBuffer, Mesh, MeshDecoder};
    let mut limits = DecodeLimits::default();
    limits.max_points = 1_000_000;
    limits.max_faces = 2_000_000;
    limits.max_decoded_bytes = 64 * 1024 * 1024;
    let mut buffer = DecoderBuffer::new(bytes).with_limits(limits);
    let mut mesh = Mesh::new();
    MeshDecoder::new()
        .decode(&mut buffer, &mut mesh)
        .map_err(|_| Error("Draco decode failed"))?;
    let attribute = |name: &str, components: usize| -> Result<Vec<f32>> {
        let Some(id) = d["attributes"][name].as_u64() else {
            return Ok(vec![]);
        };
        let a = mesh
            .attribute_by_unique_id(
                u32::try_from(id).map_err(|_| Error("Invalid Draco attribute ID"))?,
            )
            .ok_or(Error("Missing Draco attribute"))?;
        if a.num_components() as usize != components {
            return Err(Error("Wrong Draco attribute shape"));
        }
        Ok(a.read_f32s(mesh.num_points(), components))
    };
    let indices = mesh
        .faces()
        .iter()
        .flat_map(|f| f.iter().map(|p| p.0))
        .collect();
    Ok((
        attribute("POSITION", 3)?,
        attribute("NORMAL", 3)?,
        attribute(&format!("TEXCOORD_{uv_set}"), 2)?,
        indices,
    ))
}

/// Discover resources before synchronous decode, so browser I/O stays async.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn external_resources(bytes: &[u8], base: &Url) -> Result<Vec<Url>> {
    let (gltf, _) = prepare(bytes, DMat4::IDENTITY)?;
    let mut urls = Vec::new();
    let buffers = gltf.buffers().filter_map(|b| match b.source() {
        gltf::buffer::Source::Uri(uri) => Some(uri),
        _ => None,
    });
    let images = gltf.images().filter_map(|i| match i.source() {
        gltf::image::Source::Uri { uri, .. } => Some(uri),
        _ => None,
    });
    for uri in buffers
        .chain(images)
        .filter(|uri| !uri.starts_with("data:"))
    {
        let url = crate::resolve(base, uri)?;
        if !urls.contains(&url) {
            urls.push(url);
        }
        if urls.len() > 256 {
            return Err(Error("Too many external tile resources"));
        }
    }
    Ok(urls)
}
