struct U {
    res: vec2f,
    half: vec2f,
    radius: f32,
    border: f32,
    _p0: f32,
    _p1: f32,
    fill: vec4f,
    border_color: vec4f,
}
@group(0) @binding(0) var<uniform> u: U;

struct VOut {
    @builtin(position) clip: vec4f,
    @location(0) local: vec2f,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    var c = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(-1.0, 1.0),
        vec2f(-1.0, 1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
    );
    let p = c[vi];
    let px = u.res * 0.5 + p * u.half;
    var o: VOut;
    o.clip = vec4f(px.x / u.res.x * 2.0 - 1.0, 1.0 - px.y / u.res.y * 2.0, 0.0, 1.0);
    o.local = p * u.half;
    return o;
}

fn sd_round_box(p: vec2f, b: vec2f, r: f32) -> f32 {
    let q = abs(p) - b + vec2f(r);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4f {
    let d = sd_round_box(in.local, u.half, u.radius);
    let aa = max(fwidth(d), 0.001);
    let coverage = 1.0 - smoothstep(-aa, aa, d);
    let edge = smoothstep(-u.border - aa, -u.border + aa, d);
    let col = mix(u.fill, u.border_color, edge);
    let a = col.a * coverage;
    return vec4f(col.rgb * a, a); // premultiplied: the contract every shader honours (ADR-001)
}
