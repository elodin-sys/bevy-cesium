use super::*;
use glam::{DMat4, DVec3};
#[test]
fn google_parameters_stay_on_google_tiles_and_child_session_wins() {
    let root =
        Url::parse("https://tile.googleapis.com/v1/3dtiles/root.json?key=secret&session=old")
            .unwrap();
    let child = resolve(&root, "/v1/3dtiles/files/child.json?session=new").unwrap();
    assert_eq!(
        child.query_pairs().find(|(k, _)| k == "key").unwrap().1,
        "secret"
    );
    assert_eq!(
        child.query_pairs().find(|(k, _)| k == "session").unwrap().1,
        "new"
    );
    for url in [
        "https://example.com/tile.glb",
        "http://tile.googleapis.com/v1/3dtiles/tile.glb",
        "https://tile.googleapis.com/other",
    ] {
        assert!(resolve(&root, url).unwrap().query().is_none());
    }
}

#[test]
fn external_tileset_composes_transform_and_limits_recursion() {
    let json = br#"{"root":{"boundingVolume":{"sphere":[0,0,0,1]},"geometricError":0,"content":{"uri":"mesh.glb"},"transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,20,0,1]}}"#;
    let base = Url::parse("https://example.com/a/child.json").unwrap();
    let nodes = content::external_tree(
        json,
        DMat4::from_translation(DVec3::X * 6_378_137.),
        true,
        2,
        &base,
    )
    .unwrap()
    .unwrap();
    assert_eq!(nodes[0].center, DVec3::new(6_378_137., 20., 0.));
    assert!(nodes[0].add);
    assert_eq!(nodes[0].depth, 3);
    assert_eq!(
        nodes[0].url.as_ref().unwrap().as_str(),
        "https://example.com/a/mesh.glb"
    );
    assert!(content::external_tree(json, DMat4::IDENTITY, false, 48, &base).is_err());
}

#[test]
fn geographic_region_is_not_transformed_again() {
    let json = br#"{"root":{"boundingVolume":{"region":[0,0,0,0,10,10]},"geometricError":0,"transform":[1,0,0,0,0,1,0,0,0,0,1,0,1000,2000,3000,1]}}"#;
    let nodes = parse_tileset(json, &Url::parse("https://example.com/a.json").unwrap()).unwrap();
    assert!((nodes[0].center.x - 6_378_147.).abs() < 1e-6);
    assert_eq!(nodes[0].center.y, 0.);
}

#[test]
fn visible_copyright_is_counted_escaped_and_removed_on_eviction() {
    let mut s = session();
    s.resident.insert(0, (8, 0));
    s.tile_credits.insert(0, vec!["A & B <script>".into()]);
    let frame = s.update(&[view()], 0.).unwrap();
    assert_eq!(frame.credits, ["A &amp; B &lt;script&gt;"]);
    s.cache_bytes = 0;
    assert!(s.update(&[], 0.).unwrap().credits.is_empty());
    assert!(s.tile_credits.is_empty());
}

#[test]
fn nested_tilesets_stream_geometry_and_copyright_end_to_end() {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    let dir = std::env::temp_dir().join(format!(
        "caevy-nested-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let mut root: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../tests/data/tileset.json")).unwrap();
    root["root"]["content"]["uri"] = "child.json".into();
    let child = serde_json::json!({"asset":{"version":"1.1"},"root":{"boundingVolume":{"sphere":[0,0,0,20]},"geometricError":0,"content":{"uri":"triangle.gltf"}}});
    let mut gltf: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../tests/data/triangle.gltf")).unwrap();
    gltf["asset"]["copyright"] = "Provider A; Provider B".into();
    for (file, value) in [
        ("root.json", root),
        ("child.json", child),
        ("triangle.gltf", gltf),
    ] {
        std::fs::write(dir.join(file), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let mut s =
        Session::open_url(Url::from_file_path(dir.join("root.json")).unwrap().as_str()).unwrap();
    let v = View {
        position: [6_378_200., 5., -5.],
        direction: [-1., 0., 0.],
        up: [0., 0., 1.],
        ..view()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let f = s.update(&[v], 0.016).unwrap();
        assert!(!f.load_failed, "{:?}", f.error);
        if !f.loaded.is_empty() {
            assert_eq!(s.stats.requests, 2);
            assert_eq!(f.credits, ["Provider A", "Provider B"]);
            assert!((f.loaded[0].primitives[0].transform[12] - 6_378_137.).abs() < 1e-8);
            break;
        }
        assert!(Instant::now() < deadline, "nested fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn terrain_and_matching_imagery_stream_end_to_end() {
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    let dir = std::env::temp_dir().join(format!(
        "caevy-terrain-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("10/1024")).unwrap();
    let a = terrain::TileAddress {
        level: 10,
        x: 1024,
        y: 512,
    };
    std::fs::write(dir.join("tile.terrain"), terrain::tests::fixture(a)).unwrap();
    let mut levels = vec![serde_json::json!([]); 11];
    levels[10] = serde_json::json!([{"startX":1024,"startY":512,"endX":1024,"endY":512}]);
    let layer = serde_json::json!({"format":"quantized-mesh-1.0","maxzoom":10,"tiles":["tile.terrain"],"available":levels,"attribution":"Terrain test"});
    std::fs::write(dir.join("layer.json"), serde_json::to_vec(&layer).unwrap()).unwrap();
    image::RgbaImage::from_raw(1, 1, vec![12, 34, 56, 255])
        .unwrap()
        .save(dir.join("10/1024/512.png"))
        .unwrap();
    let base = Url::from_directory_path(&dir).unwrap();
    let imagery = terrain::Imagery {
        url_template: format!("{base}{{z}}/{{x}}/{{y}}.png"),
        credit: "Imagery test".into(),
    };
    let mut s = Session::open_terrain_url(base.join("layer.json").unwrap().as_str(), Some(imagery))
        .unwrap();
    let region = a.region();
    let p = geography::ecef(
        (region[0] + region[2]) * 0.5,
        (region[1] + region[3]) * 0.5,
        5000.,
    );
    let v = View {
        position: p.to_array(),
        direction: (-p.normalize()).to_array(),
        up: [0., 0., 1.],
        ..view()
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let f = s.update(&[v], 0.016).unwrap();
        assert!(!f.load_failed, "{:?}", f.error);
        if !f.loaded.is_empty() {
            assert_eq!(f.loaded[0].primitives[0].pixels, [12, 34, 56, 255]);
            assert!(f.credits.contains(&"Terrain test".into()));
            assert!(f.credits.contains(&"Imagery test".into()));
            break;
        }
        assert!(Instant::now() < deadline, "terrain fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::remove_dir_all(dir).unwrap();
}
fn view() -> View {
    View {
        position: [0., 0., 20.],
        direction: [0., 0., -1.],
        up: [0., 1., 0.],
        width: 1000.,
        height: 1000.,
        horizontal_fov: 1.,
        vertical_fov: 1.,
    }
}
fn node() -> Node {
    Node {
        terrain: None,
        depth: 0,
        transform: DMat4::IDENTITY,
        center: DVec3::ZERO,
        radius: 2.,
        error: 10.,
        children: vec![],
        url: Some(Url::parse("http://localhost/tile.glb").unwrap()),
        add: false,
    }
}
fn session() -> Session {
    let (tx, rx) = mpsc::sync_channel(4);
    Session {
        nodes: vec![node()],
        tx,
        rx,
        pending: HashSet::new(),
        failed: HashSet::new(),
        retry_after: HashMap::new(),
        elapsed: 0.0,
        resident: HashMap::new(),
        render_ready: None,
        tile_credits: HashMap::new(),
        clock: 0,
        root_pending: false,
        root_failed: false,
        client: transport::Loader::default(),
        stats: Statistics::default(),
        maximum_screen_error: 16.,
        cache_bytes: 1024,
    }
}
#[test]
fn prepared_tiles_progress_while_provider_slots_are_busy() {
    let root = Url::from_file_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/tileset.json"),
    )
    .unwrap();
    let mut package = package::Package::new(&root, Some("unused")).unwrap();
    let remote_root = "https://tile.googleapis.com/v1/3dtiles/root.json";
    package
        .install(
            &serde_json::to_vec(&serde_json::json!({
                "version": 1, "ion_asset_id": 2275207, "root": remote_root,
                "resources": {remote_root: "tileset.json"}
            }))
            .unwrap(),
        )
        .unwrap();
    let mut s = session();
    s.client = transport::Loader::prepared(package);
    s.nodes[0].url = None;
    s.nodes[0].children = (1..=5).collect();
    for index in 1..=5 {
        let mut child = node();
        child.url = Some(if index == 5 {
            root.join("triangle.gltf").unwrap()
        } else {
            Url::parse(&format!(
                "https://tile.googleapis.com/v1/3dtiles/{index}.glb"
            ))
            .unwrap()
        });
        s.nodes.push(child);
    }
    // Model three outstanding provider requests, without any external traffic.
    s.pending.extend([1, 2, 3]);
    s.update(&[view()], 0.).unwrap();
    assert!(
        s.pending.contains(&5),
        "local tile was starved by provider work"
    );
    assert!(
        !s.pending.contains(&4),
        "provider work consumed the reserved slot"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let frame = s.update(&[view()], 0.01).unwrap();
        if frame.visible.contains(&5) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "prepared geometry never became visible"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(s.stats.requests, 1);
}

#[test]
fn prepared_detail_survives_missing_provider_siblings() {
    let root = Url::parse("file:///prepared/tileset.json").unwrap();
    let mut s = session();
    s.client = transport::Loader::prepared(package::Package::new(&root, Some("unused")).unwrap());
    s.nodes[0].url = Some(root.join("parent.glb").unwrap());
    s.nodes[0].children = vec![1, 2];
    s.nodes.extend([node(), node()]);
    s.nodes[1].url = Some(root.join("detail.glb").unwrap());
    s.nodes[2].url =
        Some(Url::parse("https://tile.googleapis.com/v1/3dtiles/missing.glb").unwrap());
    s.resident.insert(0, (10, 0));
    s.pending.extend([1, 2]);

    // Keep the parent while its prepared detail is still being decoded.
    assert_eq!(s.update(&[view()], 0.).unwrap().visible, [0]);
    s.pending.remove(&1);
    s.resident.insert(1, (10, 0));
    assert_eq!(s.update(&[view()], 0.).unwrap().visible, [1]);

    // An outage outside the package must not downgrade the prepared detail.
    s.pending.remove(&2);
    s.failed.insert(2);
    assert_eq!(s.update(&[view()], 0.).unwrap().visible, [1]);

    // If no prepared detail is in view, retain the available coarse parent.
    s.nodes[1].center = DVec3::new(0., 0., 200.);
    assert_eq!(s.update(&[view()], 0.).unwrap().visible, [0]);
    s.nodes[1].center = DVec3::ZERO;

    // Switch to the complete child selection after provider recovery.
    s.failed.remove(&2);
    s.resident.insert(2, (10, 0));
    assert_eq!(s.update(&[view()], 0.).unwrap().visible, [1, 2]);
    assert_eq!(s.stats.requests, 0);
}

#[test]
fn transient_failures_retry_after_delay_but_invalid_content_stays_failed() {
    let mut s = session();
    s.tx.send(Completion::Content(
        0,
        Err(Error::Message("HTTP request failed")),
        0,
    ))
    .unwrap();
    s.update(&[], 0.).unwrap();
    assert!(s.failed.contains(&0));
    s.update(&[], 9.).unwrap();
    assert!(s.failed.contains(&0));
    s.update(&[], 1.).unwrap();
    assert!(!s.failed.contains(&0));
    s.tx.send(Completion::Content(
        0,
        Err(Error::Message("Invalid glTF")),
        0,
    ))
    .unwrap();
    s.update(&[], 0.).unwrap();
    s.update(&[], 30.).unwrap();
    assert!(s.failed.contains(&0));
}
#[test]
fn camera_validation_and_sse() {
    let v = view();
    let n = node();
    let near = n.screen_error(&v).unwrap();
    let mut far = v;
    far.position[2] = 200.;
    assert!(n.screen_error(&far).unwrap() < near);
    let mut behind = v;
    behind.direction[2] = 1.;
    assert!(n.screen_error(&behind).is_none());
    let mut invalid = v;
    invalid.up = invalid.direction;
    assert!(!invalid.valid());
    invalid = v;
    invalid.width = f64::NAN;
    assert!(!invalid.valid());
}

#[test]
fn unlit_and_linear_color_factors_survive_decode() {
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/data/triangle.gltf")).unwrap();
    let url = Url::parse("https://example.org/scene/tile.gltf").unwrap();
    let decode_fixture = |value: &serde_json::Value| {
        content::decode_with(
            &serde_json::to_vec(value).unwrap(),
            DMat4::IDENTITY,
            &url,
            &|_| panic!("Embedded fixture must not access the network"),
        )
        .unwrap()
    };
    let lit = decode_fixture(&fixture);
    assert!(!lit[0].unlit);
    fixture["materials"][0]["extensions"] = serde_json::json!({"KHR_materials_unlit": {}});
    fixture["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"] =
        serde_json::json!([0.2, 0.5, 0.9, 1.0]);
    let unlit = decode_fixture(&fixture);
    assert!(unlit[0].unlit);
    assert_eq!(unlit[0].base_color, [0.2, 0.5, 0.9, 1.0]);
    assert_eq!(unlit[0].pixels, lit[0].pixels);
}

#[test]
fn browser_prefetch_resolves_external_resources_without_network() {
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/data/triangle.gltf")).unwrap();
    let base = Url::parse("https://example.org/scene/tile.gltf").unwrap();
    assert!(
        content::external_resources(&serde_json::to_vec(&fixture).unwrap(), &base)
            .unwrap()
            .is_empty()
    );
    fixture["buffers"][0]["uri"] = serde_json::json!("mesh.bin");
    let resources =
        content::external_resources(&serde_json::to_vec(&fixture).unwrap(), &base).unwrap();
    assert_eq!(
        resources,
        [Url::parse("https://example.org/scene/mesh.bin").unwrap()]
    );
    fixture["buffers"][0]["uri"] = serde_json::json!("file:///private.bin");
    assert!(content::external_resources(&serde_json::to_vec(&fixture).unwrap(), &base).is_err());
}
#[test]
fn replacement_waits_for_every_visible_child() {
    let mut s = session();
    s.nodes[0].children = vec![1, 2];
    s.nodes.extend([node(), node()]);
    s.resident.insert(0, (10, 0));
    s.resident.insert(1, (10, 0));
    let mut wanted = vec![];
    let mut visible = vec![];
    assert!(s.select(
        0,
        &[view()],
        &mut wanted,
        &mut visible,
        &mut HashSet::new(),
        false
    ));
    assert_eq!(visible, vec![0]);
    s.resident.insert(2, (10, 0));
    visible.clear();
    s.select(
        0,
        &[view()],
        &mut wanted,
        &mut visible,
        &mut HashSet::new(),
        false,
    );
    assert_eq!(visible, vec![1, 2]);
}
#[test]
fn decoded_children_wait_for_render_preparation_without_eviction() {
    let mut s = session();
    s.cache_bytes = 0;
    s.nodes[0].children = vec![1, 2];
    s.nodes.extend([node(), node()]);
    s.resident
        .extend([(0, (10, 0)), (1, (10, 0)), (2, (10, 0))]);
    s.set_render_ready([0, 1]);
    for _ in 0..3 {
        let f = s.update(&[view()], 0.1).unwrap();
        assert_eq!(f.visible, [0]);
        assert!(f.removed.is_empty());
        assert!(!f.selection_complete);
        assert_eq!(s.stats.requests, 0);
    }
    s.set_render_ready([0, 1, 2]);
    let f = s.update(&[view()], 0.1).unwrap();
    assert_eq!(f.visible, [1, 2]);
    assert_eq!(f.removed, [0]);
    assert!(f.selection_complete);
}

#[test]
fn ready_regions_refine_while_a_sibling_keeps_its_coarse_coverage() {
    let mut s = session();
    s.nodes[0].url = None;
    s.nodes[0].children = vec![1, 2];
    s.nodes
        .extend([node(), node(), node(), node(), node(), node()]);
    s.nodes[1].children = vec![3, 4];
    s.nodes[2].children = vec![5, 6];
    s.resident.extend((1..=6).map(|id| (id, (10, 0))));
    s.set_render_ready([1, 2, 3, 4, 5]);
    let f = s.update(&[view()], 0.1).unwrap();
    assert_eq!(f.visible, [3, 4, 2]);
    assert!(!f.selection_complete);
    s.set_render_ready(1..=6);
    assert_eq!(s.update(&[view()], 0.1).unwrap().visible, [3, 4, 5, 6]);
}

#[test]
fn zoom_out_preserves_children_until_parent_is_render_ready() {
    let mut s = session();
    s.cache_bytes = 0;
    s.nodes[0].children = vec![1, 2];
    s.nodes.extend([node(), node()]);
    s.resident.extend([(1, (10, 0)), (2, (10, 0))]);
    s.pending.insert(0);
    s.set_render_ready([1, 2]);
    s.maximum_screen_error = 1e9;
    let f = s.update(&[view()], 0.1).unwrap();
    assert_eq!(f.visible, [1, 2]);
    assert!(f.removed.is_empty());
    assert!(!f.selection_complete);
    s.pending.remove(&0);
    s.resident.insert(0, (10, 0));
    let f = s.update(&[view()], 0.1).unwrap();
    assert_eq!(f.visible, [1, 2]);
    assert!(f.removed.is_empty());
    s.set_render_ready([0, 1, 2]);
    let f = s.update(&[view()], 0.1).unwrap();
    assert_eq!(f.visible, [0]);
    assert_eq!(f.removed.len(), 2);
    assert!(f.selection_complete);
}

#[test]
fn failed_sibling_keeps_parent_coverage() {
    let mut s = session();
    s.nodes[0].children = vec![1, 2];
    s.nodes.extend([node(), node()]);
    s.resident.extend([(0, (10, 0)), (1, (10, 0))]);
    s.set_render_ready([0, 1]);
    s.failed.insert(2);
    for _ in 0..3 {
        let f = s.update(&[view()], 100.).unwrap();
        assert_eq!(f.visible, [0]);
        assert!(f.load_failed);
        assert!(!f.selection_complete);
    }
}

#[test]
fn pan_away_and_back_reuses_resident_content_within_budget() {
    let mut s = session();
    s.resident.insert(0, (100, 0));
    s.set_render_ready([0]);
    let mut away = view();
    away.direction[2] = 1.;
    for _ in 0..10 {
        let f = s.update(&[away], 0.1).unwrap();
        assert!(f.visible.is_empty());
        assert!(f.removed.is_empty());
        assert_eq!(s.update(&[view()], 0.1).unwrap().visible, [0]);
    }
    assert_eq!(s.stats.requests, 0);
    assert_eq!(s.stats.resident_bytes, 100);
}

#[test]
fn requests_prioritize_coarse_coverage_then_nearby_siblings() {
    let mut s = session();
    let fixture = Url::from_file_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/triangle.gltf"),
    )
    .unwrap();
    s.nodes[0].url = Some(fixture.clone());
    s.nodes[0].children = (1..=5).rev().collect();
    for i in 1..=5 {
        let mut child = node();
        child.depth = 1;
        child.center.z = -(i as f64);
        child.url = Some(fixture.clone());
        s.nodes.push(child);
    }
    // A nearer grandchild must not overtake coarse sibling coverage.
    s.nodes[5].children = vec![6];
    let mut detail = node();
    detail.depth = 2;
    detail.center.z = 5.;
    detail.url = Some(fixture);
    s.nodes.push(detail);
    s.update(&[view()], 0.).unwrap();
    assert_eq!(s.pending, HashSet::from([0, 1, 2, 3]));
    assert_eq!(s.stats.requests, 4);
}

#[test]
fn request_distance_ignores_cameras_looking_away_from_the_tile() {
    let mut s = session();
    s.nodes.push(node());
    s.nodes[1].center.z = -100.;
    let mut away = view();
    away.position[2] = -103.;
    assert!(s.nodes[1].screen_error(&away).is_none());
    let mut wanted = vec![1, 0];
    s.prioritize_requests(&mut wanted, &[view(), away]);
    assert_eq!(wanted, [0, 1]);
}

#[test]
fn multiple_cameras_union_and_additive_refinement() {
    let mut s = session();
    s.nodes[0].add = true;
    s.nodes[0].children = vec![1];
    s.nodes.push(node());
    s.resident.insert(0, (10, 0));
    s.resident.insert(1, (10, 0));
    let mut away = view();
    away.direction[2] = 1.;
    let mut wanted = vec![];
    let mut visible = vec![];
    s.select(
        0,
        &[away, view()],
        &mut wanted,
        &mut visible,
        &mut HashSet::new(),
        false,
    );
    assert_eq!(visible, vec![1, 0]);
}
#[test]
fn cache_eviction_and_disabled_cameras() {
    let mut s = session();
    s.resident.insert(0, (4096, 0));
    let f = s.update(&[], 0.1).unwrap();
    assert!(f.visible.is_empty());
    assert_eq!(f.removed, vec![0]);
    assert_eq!(s.stats.resident_bytes, 0);
}

#[test]
fn ready_children_survive_while_parent_covers_pending_siblings() {
    let mut s = session();
    s.cache_bytes = 0;
    s.nodes[0].children = vec![1, 2];
    s.nodes.extend([node(), node()]);
    s.resident.insert(0, (100, 0));
    s.resident.insert(1, (100, 0));
    // An outstanding sibling request keeps the coarse parent on screen.
    s.pending.insert(2);
    for _ in 0..5 {
        let frame = s.update(&[view()], 0.1).unwrap();
        assert_eq!(frame.visible, vec![0]);
        assert!(frame.removed.is_empty());
        assert!(s.resident.contains_key(&1));
        assert_eq!(s.stats.requests, 0);
    }
    s.pending.remove(&2);
    s.resident.insert(2, (100, 0));
    let frame = s.update(&[view()], 0.1).unwrap();
    assert_eq!(frame.visible, vec![1, 2]);
    assert_eq!(frame.removed, vec![0]);
}

#[test]
fn unused_cache_budget_is_independent_of_visible_working_set() {
    let mut s = session();
    s.cache_bytes = 100;
    s.nodes.extend([node(), node()]);
    s.resident.insert(0, (1000, 0));
    s.resident.insert(1, (60, 1));
    s.resident.insert(2, (60, 2));
    let frame = s.update(&[view()], 0.1).unwrap();
    assert_eq!(frame.visible, vec![0]);
    assert_eq!(frame.removed, vec![1]);
    assert!(s.resident.contains_key(&2));
    assert_eq!(s.stats.resident_bytes, 1060);
    s.cache_bytes = 0;
    let frame = s.update(&[], 0.1).unwrap();
    assert_eq!(frame.removed.len(), 2);
    assert_eq!(s.stats.resident_bytes, 0);
}
#[test]
fn transforms_compose_in_double_precision_and_resolve_urls() {
    let data = serde_json::json!({"root":{"boundingVolume":{"sphere":[0,0,0,2]},"geometricError":0,"transform":DMat4::from_translation(DVec3::new(6378137.,0.,0.)).to_cols_array(),"children":[{"boundingVolume":{"sphere":[0,0,0,1]},"geometricError":0,"transform":DMat4::from_translation(DVec3::new(0.125,0.,0.)).to_cols_array(),"content":{"uri":"../tile.glb"}}]}});
    let nodes = parse_tileset(
        &serde_json::to_vec(&data).unwrap(),
        &Url::parse("https://example.org/scene/tileset.json").unwrap(),
    )
    .unwrap();
    assert_eq!(nodes[1].center.x, 6378137.125);
    assert_eq!(
        nodes[1].url.as_ref().unwrap().as_str(),
        "https://example.org/tile.glb"
    );
}
#[test]
fn malformed_payloads_fail_without_panicking() {
    let u = Url::parse("file:///invalid.glb").unwrap();
    let a = ureq::Agent::new_with_defaults();
    for bytes in [&b"b3dm"[..], &b"glTF"[..], &b"{}"[..]] {
        assert!(decode(bytes, DMat4::IDENTITY, &u, &a).is_err());
    }
    let mut b = vec![0; 28];
    b[..4].copy_from_slice(b"b3dm");
    b[4..8].copy_from_slice(&1u32.to_le_bytes());
    b[8..12].copy_from_slice(&28u32.to_le_bytes());
    b[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&b, DMat4::IDENTITY, &u, &a).is_err());
}
#[test]
fn unsupported_features_are_explicit() {
    assert!(Session::open_ion(-1, "secret").is_err());
    assert!(Session::open_url("ftp://example.org/tileset.json").is_err());
    let remote = Url::parse("https://example.org/tileset.json").unwrap();
    assert!(resolve(&remote, "file:///etc/passwd").is_err());
}
#[test]
fn covered_ancestors_are_not_downloaded_again() {
    let mut s = session();
    s.nodes[0].children = vec![1, 2];
    s.nodes.extend([node(), node()]);
    s.resident.insert(1, (10, 0));
    s.resident.insert(2, (10, 0));
    let mut wanted = vec![];
    let mut visible = vec![];
    s.select(
        0,
        &[view()],
        &mut wanted,
        &mut visible,
        &mut HashSet::new(),
        false,
    );
    assert_eq!(visible, vec![1, 2]);
    assert!(wanted.is_empty());
}
#[test]
fn child_failure_preserves_parent_and_reports_error() {
    let mut s = session();
    s.nodes[0].children = vec![1];
    s.nodes.push(node());
    s.resident.insert(0, (10, 0));
    s.tx.send(Completion::Content(
        1,
        Err(Error::Message("HTTP request failed")),
        0,
    ))
    .unwrap();
    let f = s.update(&[view()], 0.1).unwrap();
    assert_eq!(f.visible, vec![0]);
    assert!(f.load_failed);
    assert_eq!(f.pending, 0);
}

#[test]
fn hierarchy_errors_are_distinct_and_leave_the_session_usable() {
    let mut s = session();
    s.nodes.extend((1..100_000).map(|_| node()));
    for (id, nodes, expected) in [
        (1, vec![], "Empty external tileset"),
        (2, vec![node()], "Tileset hierarchy limit exceeded"),
    ] {
        s.tx.send(Completion::Content(id, Ok(Payload::Hierarchy(nodes)), 0))
            .unwrap();
        let frame = s.update(&[], 0.).unwrap();
        assert_eq!(frame.error, Some(expected));
        assert!(frame.load_failed);
        assert_eq!(s.nodes.len(), 100_000);
    }
    assert_eq!(s.stats.failed_tiles, 2);
    for _ in 0..1000 {
        assert!(s.update(&[], 0.016).unwrap().load_failed);
    }
}
