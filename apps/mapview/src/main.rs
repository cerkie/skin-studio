//! Headless gameplay snapshot: renders a map with a skin at the given times to PNG files.
//! usage: mapview [--map file.osu | --random SEED] [--skin dir|file.osk] [--follow settings.json] [--t 1000,2000] [--out prefix]
use std::path::PathBuf;

use game::beatmap::Beatmap;
use game::scene::{Frame, FollowFrames, Scene};
use game::skin::{self, Sheet, Skin};
use preview::game_renderer::GameRenderer;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

fn main() {
    pollster::block_on(run());
}

async fn run() {
    let songs = PathBuf::from(std::env::var("OSU_SONGS").unwrap_or_default());
    let map = if let Some(p) = arg("--map") {
        Beatmap::parse_file(&PathBuf::from(p)).expect("map")
    } else {
        let seed = arg("--random").and_then(|s| s.parse().ok()).unwrap_or(1u64);
        let (p, bm) = game::library::random_map(&songs, seed).expect("no map found");
        println!("map: {}", p.display());
        bm
    };
    println!("{} [{}]  objects {}  CS {} AR {}", map.title, map.version, map.objects.len(), map.cs, map.ar);
    let skin_path = arg("--skin").unwrap_or_else(|| "argon".into());
    let skin = Skin::open(&PathBuf::from(skin_path)).expect("skin");
    println!("skin: {}", skin.name);

    let ss = 2.0f32;
    let scale = engine::lazer::circle_scale(map.cs);
    let mut sheet = if matches!(skin.style, skin::Style::Argon { .. }) {
        let mut s = Sheet::default();
        game::argon::add_to_sheet(&mut s, scale, ss);
        s
    } else {
        Sheet::pack(&skin::gameplay_entries(&skin, scale, ss))
    };

    // optional follow-point animation, generated in memory and appended below the skin elements
    let mut follow = None;
    if let Some(p) = arg("--follow") {
        let s = engine::settings::Settings::from_json_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        let frames = engine::gen::build_frames(&s, 1, &Default::default());
        let atlas = engine::atlas::Atlas::from_frames(&frames, s.fps);
        let y0 = sheet.h;
        let w = sheet.w.max(atlas.width);
        let mut rgba = vec![0u8; (w * (y0 + atlas.height) * 4) as usize];
        for row in 0..sheet.h {
            let n = (sheet.w * 4) as usize;
            rgba[(row * w * 4) as usize..][..n].copy_from_slice(&sheet.rgba[(row * sheet.w * 4) as usize..][..n]);
        }
        for row in 0..atlas.height {
            let n = (atlas.width * 4) as usize;
            rgba[((y0 + row) * w * 4) as usize..][..n].copy_from_slice(&atlas.rgba[(row * atlas.width * 4) as usize..][..n]);
        }
        let rects = atlas.frames.iter().map(|f| f.origin.map(|(ox, oy)| skin::Rect { x: ox, y: y0 + oy, w: f.w, h: f.h })).collect();
        follow = Some(FollowFrames { rects, fps: atlas.fps, ratio: 1.0, ini_set: true, additive: false });
        sheet.rgba = rgba;
        sheet.w = w;
        sheet.h += atlas.height;
    }
    println!("sheet {}x{} with {} elements", sheet.w, sheet.h, sheet.elems.len());
    let (map, skin, sheet) = (std::sync::Arc::new(map), std::sync::Arc::new(skin), std::sync::Arc::new(sheet));
    let scene = Scene::new(map.clone(), skin.clone(), sheet.clone(), follow);

    let instance = wgpu::Instance::default();
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }).await.expect("gpu");
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() }).await.expect("device");
    let (w, h) = (1280u32, 960u32);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut r = GameRenderer::new(device.clone(), queue.clone(), format, (w, h), &sheet.rgba, (sheet.w, sheet.h), &scene.geoms, ss);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let out = arg("--out").unwrap_or_else(|| "mapview".into());
    let first = map.objects.first().map(|o| o.time).unwrap_or(0.0);
    let spin = map.objects.iter().find(|o| matches!(o.kind, game::beatmap::Kind::Spinner)).map(|o| (o.time, o.end_time));
    if std::env::args().any(|a| a == "--spin") && spin.is_none() {
        println!("no spinner in this map");
        return;
    }
    let rep = map.objects.iter().find_map(|o| match &o.kind { game::beatmap::Kind::Slider(s) if s.slides >= 2 => Some((o.time, o.end_time)), _ => None });
    let times: Vec<f32> = if std::env::args().any(|a| a == "--repeat") {
        let (s, e) = rep.expect("no repeat slider in this map");
        vec![s - 200.0, s + (e - s) * 0.45, e - (e - s) * 0.1]
    } else if std::env::args().any(|a| a == "--spin") {
        let (s, e) = spin.unwrap();
        vec![s - 300.0, s + 700.0, s + (e - s) * 0.85, e + 100.0]
    } else {
        arg("--t").map(|s| s.split(',').filter_map(|v| v.parse().ok()).collect()).unwrap_or_else(|| vec![first + 600.0])
    };
    let mut frame = Frame::default();
    for (i, t) in times.iter().enumerate() {
        let t0 = std::time::Instant::now();
        scene.build(*t, &mut frame);
        let build = t0.elapsed();
        r.render(&view, &frame, &scene.geoms, [0.06, 0.06, 0.08, 1.0]);
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (w * h * 4) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        let bytes = slice.get_mapped_range().expect("range").to_vec();
        let name = format!("{out}_{i}.png");
        let file = std::fs::File::create(&name).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&bytes).unwrap();
        println!("t={t} ms: {} sprites, {} sliders, scene {:?} -> {name}", frame.sprites.len(), frame.sliders.len(), build);
    }
}
