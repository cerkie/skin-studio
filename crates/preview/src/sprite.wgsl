// One instanced draw for the whole preview: follow-point sprites from the frame atlas, and hit circles as
// anti-aliased signed-distance shapes. Output is premultiplied alpha.
struct Globals {
    viewport: vec2<f32>,
    atlas: vec2<f32>,
};
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_smp: sampler;

struct Inst {
    @location(0) pos: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) rot_alpha: vec2<f32>,   // rotation (radians), opacity
    @location(3) uv: vec4<f32>,          // atlas rect in pixels: min.xy, max.xy
    @location(4) color: vec4<f32>,
    @location(5) params: vec4<f32>,      // x: kind (0 sprite, 1 disc, 2 ring), y: ring width
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
    @location(2) alpha: f32,
    @location(3) color: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) half_size: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0));
    let c = corners[vi];
    let local = c * inst.half_size;
    let cs = cos(inst.rot_alpha.x);
    let sn = sin(inst.rot_alpha.x);
    let p = inst.pos + vec2<f32>(local.x * cs - local.y * sn, local.x * sn + local.y * cs);
    var o: VOut;
    o.clip = vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, 0.0, 1.0);
    o.uv = mix(inst.uv.xy, inst.uv.zw, c * 0.5 + 0.5);
    o.local = local;
    o.alpha = inst.rot_alpha.y;
    o.color = inst.color;
    o.params = inst.params;
    o.half_size = inst.half_size;
    return o;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    if v.params.x < 0.5 {
        return textureSample(atlas_tex, atlas_smp, v.uv / g.atlas) * v.alpha;
    }
    let d = length(v.local);
    let r = v.half_size.x - 1.0;
    var cover: f32;
    if v.params.x < 1.5 {
        cover = clamp(r - d + 0.5, 0.0, 1.0);                                                  // filled disc
    } else {
        cover = clamp(0.5 * v.params.y + 0.5 - abs(d - (r - 0.5 * v.params.y)), 0.0, 1.0);   // ring
    }
    let a = cover * v.color.a * v.alpha;
    return vec4<f32>(v.color.rgb * a, a);
}
