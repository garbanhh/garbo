// Un único pipeline instanciado: cada instancia es un quad.
//   kind 0 = rectángulo sólido (con radio de esquina opcional)
//   kind 1 = glifo máscara (tinte con `color`)
//   kind 2 = glifo a color (emoji)
//   kind 3 = arco (esquinas redondeadas ╭╮╰╯): uv = (centro.x, centro.y, radio, grosor)
//            en píxeles relativos a la esquina superior izquierda del quad
//   kind 4 = segmento (iconos): uv = (x0, y0, x1, y1) relativos al quad, radius = medio grosor
//   kind 5 = degradado vertical: `color` arriba, uv = color abajo
//   kind 6 = contorno de rectángulo redondeado: uv.x = grosor, radius = radio

struct Uniforms {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct Inst {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) kind: u32,
    @location(5) radius: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: u32,
    @location(3) local: vec2<f32>,
    @location(4) half_size: vec2<f32>,
    @location(5) radius: f32,
    @location(6) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let px = inst.pos + corner * inst.size;
    var out: VOut;
    out.clip = vec4<f32>(px.x / u.screen.x * 2.0 - 1.0, 1.0 - px.y / u.screen.y * 2.0, 0.0, 1.0);
    out.uv = inst.uv.xy + corner * inst.uv.zw;
    out.color = inst.color;
    out.kind = inst.kind;
    out.half_size = inst.size * 0.5;
    out.local = (corner - vec2<f32>(0.5)) * inst.size;
    out.radius = inst.radius;
    out.params = inst.uv;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    if (in.kind == 1u) {
        let a = textureSampleLevel(atlas, samp, in.uv, 0.0).a;
        return vec4<f32>(in.color.rgb, 1.0) * (in.color.a * a);
    }
    if (in.kind == 2u) {
        let c = textureSampleLevel(atlas, samp, in.uv, 0.0);
        return vec4<f32>(c.rgb * c.a, c.a) * in.color.a;
    }
    if (in.kind == 3u) {
        let p = in.local + in.half_size;
        let d = length(p - in.params.xy);
        let a = clamp(in.params.w * 0.5 - abs(d - in.params.z) + 0.5, 0.0, 1.0) * in.color.a;
        return vec4<f32>(in.color.rgb * a, a);
    }
    if (in.kind == 4u) {
        let p = in.local + in.half_size;
        let pa = p - in.params.xy;
        let ba = in.params.zw - in.params.xy;
        let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
        let d = length(pa - ba * h) - in.radius;
        let a = clamp(0.5 - d, 0.0, 1.0) * in.color.a;
        return vec4<f32>(in.color.rgb * a, a);
    }
    if (in.kind == 5u) {
        let t = clamp((in.local.y + in.half_size.y) / max(2.0 * in.half_size.y, 1.0), 0.0, 1.0);
        let c = mix(in.color, in.params, t);
        return vec4<f32>(c.rgb * c.a, c.a);
    }
    if (in.kind == 6u) {
        let q = abs(in.local) - in.half_size + vec2<f32>(in.radius);
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - in.radius;
        let t = in.params.x;
        let a = clamp(t * 0.5 - abs(d + t * 0.5) + 0.5, 0.0, 1.0) * in.color.a;
        return vec4<f32>(in.color.rgb * a, a);
    }
    var alpha = in.color.a;
    if (in.radius > 0.0) {
        // SDF de rectángulo redondeado con antialias de 1 px
        let q = abs(in.local) - in.half_size + vec2<f32>(in.radius);
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - in.radius;
        alpha = alpha * clamp(0.5 - d, 0.0, 1.0);
    }
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
