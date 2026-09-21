// Every fragment returns PREMULTIPLIED colour: the contract from ADR-001.
struct Globals {
    res: vec2f,
    _pad: vec2f,
}
@group(0) @binding(0) var<uniform> g: Globals;

fn corner(vi: u32) -> vec2f {
    var c = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(-1.0, 1.0),
        vec2f(-1.0, 1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
    );
    return c[vi];
}

fn to_ndc(p: vec2f) -> vec4f {
    return vec4f(p.x / g.res.x * 2.0 - 1.0, 1.0 - p.y / g.res.y * 2.0, 0.0, 1.0);
}

fn sd_round_box(p: vec2f, b: vec2f, r: f32) -> f32 {
    let q = abs(p) - b + vec2f(r);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sd_segment(p: vec2f, a: vec2f, b: vec2f) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let len2 = dot(ba, ba);
    var h = 0.0;
    if (len2 > 0.0) {
        h = clamp(dot(pa, ba) / len2, 0.0, 1.0);
    }
    return length(pa - ba * h);
}

fn premul(c: vec4f) -> vec4f {
    return vec4f(c.rgb * c.a, c.a);
}

// ---- shapes: rounded rect (0), capsule (1), soft shadow (2) ----

struct ShapeIn {
    @location(0) a: vec2f,
    @location(1) b: vec2f,
    @location(2) radius: f32,
    @location(3) border: f32,
    @location(4) kind: f32,
    @location(5) soft: f32,
    @location(6) fill_top: vec4f,
    @location(7) fill_bot: vec4f,
    @location(8) border_color: vec4f,
    @location(9) clip: vec4f,
}

struct ShapeOut {
    @builtin(position) clip_pos: vec4f,
    @location(0) pos: vec2f,
    @location(1) @interpolate(flat) a: vec2f,
    @location(2) @interpolate(flat) b: vec2f,
    @location(3) @interpolate(flat) params: vec4f, // radius, border, kind, soft
    @location(4) @interpolate(flat) fill_top: vec4f,
    @location(5) @interpolate(flat) fill_bot: vec4f,
    @location(6) @interpolate(flat) border_color: vec4f,
    @location(7) @interpolate(flat) clip: vec4f,
}

@vertex
fn vs_shape(@builtin(vertex_index) vi: u32, in: ShapeIn) -> ShapeOut {
    let c = corner(vi);
    var lo: vec2f;
    var hi: vec2f;
    if (in.kind > 0.5 && in.kind < 1.5) {
        let m = in.radius + 2.0;
        lo = min(in.a, in.b) - vec2f(m);
        hi = max(in.a, in.b) + vec2f(m);
    } else {
        let m = 2.0 + select(0.0, in.soft * 2.0, in.kind > 1.5);
        lo = in.a - in.b - vec2f(m);
        hi = in.a + in.b + vec2f(m);
    }
    let p = mix(lo, hi, c * 0.5 + vec2f(0.5));
    var o: ShapeOut;
    o.clip_pos = to_ndc(p);
    o.pos = p;
    o.a = in.a;
    o.b = in.b;
    o.params = vec4f(in.radius, in.border, in.kind, in.soft);
    o.fill_top = in.fill_top;
    o.fill_bot = in.fill_bot;
    o.border_color = in.border_color;
    o.clip = in.clip;
    return o;
}

@fragment
fn fs_shape(in: ShapeOut) -> @location(0) vec4f {
    if (in.pos.x < in.clip.x || in.pos.y < in.clip.y || in.pos.x > in.clip.z || in.pos.y > in.clip.w) {
        discard;
    }
    let kind = in.params.z;
    if (kind > 1.5) {
        let d = sd_round_box(in.pos - in.a, in.b, in.params.x);
        let s = max(in.params.w, 0.5);
        let cov = 1.0 - smoothstep(-s, s, d);
        return premul(in.fill_top) * cov;
    }
    if (kind > 0.5) {
        let d = sd_segment(in.pos, in.a, in.b) - in.params.x;
        let cov = clamp(0.5 - d, 0.0, 1.0);
        return premul(in.fill_top) * cov;
    }
    let d = sd_round_box(in.pos - in.a, in.b, in.params.x);
    let cov = clamp(0.5 - d, 0.0, 1.0);
    let t = clamp((in.pos.y - (in.a.y - in.b.y)) / max(2.0 * in.b.y, 1.0), 0.0, 1.0);
    var col = premul(mix(in.fill_top, in.fill_bot, t));
    if (in.params.y > 0.0) {
        let bm = clamp(0.5 + (d + in.params.y), 0.0, 1.0);
        col = mix(col, premul(in.border_color), bm);
    }
    return col * cov;
}

// ---- images ----

struct ImgIn {
    @location(0) center: vec2f,
    @location(1) half: vec2f,
    @location(2) radius: f32,
    @location(3) alpha: f32,
    @location(4) tint: vec4f,
    @location(5) clip: vec4f,
}

struct ImgOut {
    @builtin(position) clip_pos: vec4f,
    @location(0) uv: vec2f,
    @location(1) pos: vec2f,
    @location(2) @interpolate(flat) center: vec2f,
    @location(3) @interpolate(flat) half: vec2f,
    @location(4) @interpolate(flat) params: vec2f,
    @location(5) @interpolate(flat) tint: vec4f,
    @location(6) @interpolate(flat) clip: vec4f,
}

@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

@vertex
fn vs_img(@builtin(vertex_index) vi: u32, in: ImgIn) -> ImgOut {
    let c = corner(vi);
    let p = in.center + c * in.half;
    var o: ImgOut;
    o.clip_pos = to_ndc(p);
    o.uv = c * 0.5 + vec2f(0.5);
    o.pos = p;
    o.center = in.center;
    o.half = in.half;
    o.params = vec2f(in.radius, in.alpha);
    o.tint = in.tint;
    o.clip = in.clip;
    return o;
}

@fragment
fn fs_img(in: ImgOut) -> @location(0) vec4f {
    if (in.pos.x < in.clip.x || in.pos.y < in.clip.y || in.pos.x > in.clip.z || in.pos.y > in.clip.w) {
        discard;
    }
    var cov = 1.0;
    if (in.params.x > 0.5) {
        cov = clamp(0.5 - sd_round_box(in.pos - in.center, in.half, in.params.x), 0.0, 1.0);
    }
    let c = textureSample(tex, samp, in.uv);
    let a = c.a * in.tint.a * in.params.y * cov;
    return vec4f(c.rgb * in.tint.rgb * a, a);
}
