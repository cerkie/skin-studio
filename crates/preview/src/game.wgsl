// Gameplay renderer: skin sprites (optionally tinted) and slider bodies with a depth buffer, premultiplied output.
struct Globals {
    viewport: vec2<f32>,
    sheet: vec2<f32>,
    scale: vec4<f32>,
};
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var sheet_tex: texture_2d<f32>;
@group(0) @binding(2) var sheet_smp: sampler;

struct Inst {
    @location(0) pos: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) rot_alpha: vec2<f32>,
    @location(3) uv: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) params: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) color: vec4<f32>,
    @location(3) tint: f32,
};

@vertex
fn vs_sprite(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0));
    let c = corners[vi];
    let local = c * inst.half_size;
    let cs = cos(inst.rot_alpha.x);
    let sn = sin(inst.rot_alpha.x);
    let p = inst.pos + vec2<f32>(local.x * cs - local.y * sn, local.x * sn + local.y * cs);
    var o: VOut;
    o.clip = vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, 0.5, 1.0);
    o.uv = mix(inst.uv.xy, inst.uv.zw, c * 0.5 + 0.5);
    o.alpha = inst.rot_alpha.y;
    o.color = inst.color;
    o.tint = inst.params.x;
    return o;
}

@fragment
fn fs_sprite(v: VOut) -> @location(0) vec4<f32> {
    var t = textureSample(sheet_tex, sheet_smp, v.uv / g.sheet);
    if v.tint > 0.5 {
        t = vec4<f32>(t.rgb * v.color.rgb, t.a);
    }
    return t * v.alpha;
}

struct SIn {
    @location(0) p: vec3<f32>,            // x, y (view units), d (0 on the path, 1 at the border)
    @location(1) border: vec4<f32>,
    @location(2) outer: vec4<f32>,
    @location(3) inner: vec4<f32>,
    @location(4) misc: vec4<f32>,         // alpha, body alpha, depth slot, depth range
};
struct SOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) d: f32,
    @location(1) border: vec4<f32>,
    @location(2) outer: vec4<f32>,
    @location(3) inner: vec3<f32>,
    @location(4) misc: vec4<f32>,
};

@vertex
fn vs_slider(i: SIn) -> SOut {
    let p = i.p.xy * g.scale.x + g.scale.yz;
    var o: SOut;
    // depth: each slider owns a slot, and inside it the path centre is nearest
    o.clip = vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, i.misc.z + i.p.z * i.misc.w, 1.0);
    o.d = i.p.z;
    o.border = i.border;
    o.outer = i.outer;
    o.inner = i.inner.rgb;
    o.misc = i.misc;
    return o;
}

@fragment
fn fs_slider(v: SOut) -> @location(0) vec4<f32> {
    let pos = 1.0 - v.d;
    let border_portion = v.border.w;
    let shadow_portion = v.outer.w;
    // one pixel of anti-aliasing on every inner edge (the outer silhouette is handled by multisampling)
    let w = max(fwidth(pos), 1e-4);
    var shadow_a = 0.0;
    if shadow_portion > 0.0 {
        shadow_a = 0.25 * clamp(pos / shadow_portion, 0.0, 1.0);
    }
    let in_border = smoothstep(shadow_portion - w, shadow_portion + w, pos) * (1.0 - smoothstep(border_portion - w, border_portion + w, pos));
    let in_body = smoothstep(border_portion - w, border_portion + w, pos);
    let u = clamp((pos - border_portion) / max(1.0 - border_portion, 1e-4), 0.0, 1.0);
    let body_rgb = mix(v.outer.rgb, v.inner, u);
    // premultiplied layers: shadow, then border, then body
    let a = shadow_a * (1.0 - in_border) * (1.0 - in_body);
    let ba = in_border;
    let ya = in_body * v.misc.y;
    let pm = v.border.rgb * ba + body_rgb * ya;
    let pa = ba + ya + a;
    return vec4<f32>(pm, min(pa, 1.0)) * v.misc.x;
}
