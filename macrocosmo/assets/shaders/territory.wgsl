#import bevy_sprite::mesh2d_vertex_output::VertexOutput

// Colony data: xy = world position, z = authority strength, w = empire_id
struct ColonyData {
    data: array<vec4<f32>, 64>,
}

// Empire colors
struct EmpireColors {
    colors: array<vec4<f32>, 4>,
}

// Parameters: x = void_constant, y = colony_count, z = empire_count, w = unused
struct Params {
    values: vec4<f32>,
}

@group(2) @binding(0) var<uniform> colony_data: ColonyData;
@group(2) @binding(1) var<uniform> empire_colors: EmpireColors;
@group(2) @binding(2) var<uniform> params: Params;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = mesh.world_position.xy;
    let colony_count = i32(params.values.y);
    let empire_count = i32(params.values.z);
    let void_constant = params.values.x;

    // Compute authority per empire (max 4 empires)
    var auth: array<f32, 4>;
    for (var e = 0; e < 4; e++) {
        auth[e] = 0.0;
    }

    for (var i = 0; i < colony_count; i++) {
        let col = colony_data.data[i];
        let diff = pixel - col.xy;
        let dist_sq = max(dot(diff, diff), 0.01);
        let empire = i32(col.w);
        if (empire >= 0 && empire < 4) {
            auth[empire] += col.z / dist_sq;
        }
    }

    // Find owner (empire with highest authority, compared against void constant)
    var max_auth = void_constant;
    var owner = -1;
    var second_max = 0.0;
    for (var e = 0; e < empire_count; e++) {
        if (auth[e] > max_auth) {
            second_max = max_auth;
            max_auth = auth[e];
            owner = e;
        } else if (auth[e] > second_max) {
            second_max = auth[e];
        }
    }

    // Void wins — fully transparent
    if (owner < 0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    // Border effect: fade near contested boundaries (avoids div-by-zero).
    let ratio = second_max / max(max_auth, 1e-6);
    let border = 1.0 - smoothstep(0.6, 0.95, ratio);

    // Edge fade: strength just above the void threshold should fade out so
    // the outer boundary of the territory is soft rather than hard-cut.
    let strength_ratio = void_constant / max(max_auth, 1e-6);
    let edge = 1.0 - smoothstep(0.6, 1.0, strength_ratio);

    // Pick owner color from the uniform palette. Fall back to a neutral grey
    // if for some reason the owner index is out of range (shouldn't happen).
    var color = vec4<f32>(0.5, 0.5, 0.5, 1.0);
    if (owner == 0) { color = empire_colors.colors[0]; }
    else if (owner == 1) { color = empire_colors.colors[1]; }
    else if (owner == 2) { color = empire_colors.colors[2]; }
    else if (owner == 3) { color = empire_colors.colors[3]; }

    // Higher alpha + near-full color intensity so territory is actually
    // readable on the star map. Border and edge modulation keep the look
    // layered instead of flat.
    let alpha = 0.45 * border * edge;
    return vec4<f32>(color.rgb * 0.9 * border * edge, alpha);
}
