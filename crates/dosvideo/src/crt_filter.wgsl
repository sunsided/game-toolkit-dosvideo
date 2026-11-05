// CRT post-filter for the emulated picture.
//
// The source texture holds one row per real scanline (400 for VGA mode 13h, 200 for CGA,
// 350 for Hercules), one column per dot. So the beam can be modeled directly: each scanline is
// a horizontal Gaussian-blurred stripe of light whose height grows with brightness, the gaps
// between stripes show up wherever the window has pixels to spare, and a phosphor mask splits
// light into R, G and B stripes or dots on the output pixels. All math runs in linear light
// (the source is sampled as sRGB, the target is an sRGB view).

struct Params {
    rect: vec4<f32>,    // x, y, w, h of the picture in window pixels
    tex: vec4<f32>,     // texture w, h; region origin x, y (dots)
    region: vec4<f32>,  // region w, h (dots); unused, unused
    beam: vec4<f32>,    // scanline strength, beam width min, beam width max, horizontal sigma
    mask: vec4<f32>,    // type (0 none, 1 grille, 2 slot, 3 dots), strength, pitch px, brightness
    fx: vec4<f32>,      // bloom, curvature, vignette, corner radius
    mono: vec4<f32>,    // phosphor color (linear), w = 1: show brightness only
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> u: Params;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn texel(x: i32, y: i32) -> vec3<f32> {
    let size = vec2<i32>(u.tex.xy);
    let c = clamp(vec2<i32>(x, y), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return textureLoad(src, c, 0).rgb;
}

// One scanline, blurred horizontally like the beam's spot.
fn scanline(x: f32, row: i32) -> vec3<f32> {
    let sigma = max(u.beam.w, 0.05);
    let x0 = floor(x - 0.5);
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var i = -3; i <= 4; i = i + 1) {
        let xi = x0 + f32(i);
        let d = (xi + 0.5) - x;
        let w = exp(-0.5 * d * d / (sigma * sigma));
        acc = acc + texel(i32(xi), row) * w;
        wsum = wsum + w;
    }
    return acc / wsum;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Vertical beam profile: brighter lines are taller.
fn beam(d: f32, c: vec3<f32>) -> vec3<f32> {
    let l = clamp(max(c.r, max(c.g, c.b)), 0.0, 1.0);
    let w = mix(u.beam.y, u.beam.z, sqrt(l));
    let k = exp(-0.5 * (d * d) / (w * w));
    return c * k;
}

fn phosphor_mask(pos: vec2<f32>) -> vec3<f32> {
    let kind = i32(u.mask.x + 0.5);
    let strength = u.mask.y;
    let pitch = max(u.mask.z, 1.0);
    if (kind == 0 || strength <= 0.0) {
        return vec3<f32>(1.0);
    }
    let dark = 1.0 - strength;
    let sub = pitch / 3.0;
    var x = pos.x;
    var gap = 1.0;
    if (kind == 2) {
        // Slot mask: every other triad column shifted down half a slot, with short horizontal
        // gaps between slots.
        let col = floor(pos.x / pitch);
        let slot_h = pitch * 1.5;
        let y = pos.y + (col % 2.0) * slot_h * 0.5;
        if (fract(y / slot_h) < 0.22) {
            gap = dark;
        }
    } else if (kind == 3) {
        // Shadow mask dot triads: rows of triads offset by half a triad.
        let row = floor(pos.y / (pitch * 0.5));
        x = x + (row % 2.0) * pitch * 0.5;
    }
    let ch = i32(floor(x / sub)) % 3;
    var m = vec3<f32>(dark);
    if (ch == 0) {
        m.r = 1.0;
    } else if (ch == 1) {
        m.g = 1.0;
    } else {
        m.b = 1.0;
    }
    return m * gap;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var uv = (pos.xy - u.rect.xy) / u.rect.zw;

    // Barrel distortion of a curved tube.
    let k = u.fx.y;
    var c = uv * 2.0 - 1.0;
    c = c * (1.0 + k * (c.yx * c.yx));
    uv = c * 0.5 + 0.5;
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    // Rounded corners.
    let r = u.fx.w;
    if (r > 0.0) {
        let aspect = u.rect.z / u.rect.w;
        let q = abs(uv - 0.5) * 2.0;
        let e = max(q - vec2<f32>(1.0 - r / aspect, 1.0 - r), vec2<f32>(0.0)) * vec2<f32>(aspect, 1.0);
        if (length(e) > r) {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
    }

    let p = u.tex.zw + uv * u.region.xy;
    let line = p.y - 0.5;
    let y0 = floor(line);
    let fy = line - y0;
    let a = scanline(p.x, i32(y0));
    let b = scanline(p.x, i32(y0) + 1);

    // Scanline gaps only where the window has pixels for them.
    let px_per_line = u.rect.w / u.region.y;
    let scan = u.beam.x * smoothstep(1.4, 2.6, px_per_line);
    let flat = select(b, a, fy < 0.5);
    let lit = beam(fy, a) + beam(1.0 - fy, b);
    // Compensate the average brightness lost to the gaps.
    let avg_w = mix(u.beam.y, u.beam.z, 0.5);
    let norm = 1.0 / clamp(avg_w * 2.5066, 0.35, 1.0);
    var col = mix(flat, lit * norm, scan);

    // Bloom: light spilling around bright areas.
    let bloom = u.fx.x;
    if (bloom > 0.0) {
        let inv = 1.0 / u.tex.xy;
        var g = vec3<f32>(0.0);
        let rx = 4.0;
        let ry = 2.5;
        for (var i = 0; i < 12; i = i + 1) {
            let ang = f32(i) * 0.5235988;
            let o = vec2<f32>(cos(ang) * rx, sin(ang) * ry);
            g = g + textureSampleLevel(src, smp, (p + o) * inv, 0.0).rgb;
            g = g + textureSampleLevel(src, smp, (p + o * 0.5) * inv, 0.0).rgb;
        }
        g = g / 24.0;
        col = col + g * g * bloom * 2.0;
    }

    // A monochrome monitor shows brightness in its phosphor color.
    if (u.mono.w > 0.5) {
        col = vec3<f32>(luma(col)) * u.mono.rgb;
    }

    // Phosphor mask, fading out when it would be finer than the window can show.
    let mask_fade = smoothstep(2.0, 3.0, u.mask.z);
    col = col * mix(vec3<f32>(1.0), phosphor_mask(pos.xy), mask_fade);
    col = col * u.mask.w;

    // Vignette.
    let v = uv * (1.0 - uv.yx);
    let vig = pow(clamp(v.x * v.y * 16.0, 0.0, 1.0), 0.25);
    col = col * mix(1.0, vig, u.fx.z);

    return vec4<f32>(col, 1.0);
}
