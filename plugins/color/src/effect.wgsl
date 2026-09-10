// The fragment entry point of the Colour Grade effect.
//
// The host prepends its own prelude, so this file declares none of it: the
// `EffectParams` uniform it builds from the parameters `describe` declares (one
// member per parameter id, so `params.exposure` below), the clip's picture as
// `source` with `source_sampler`, and the `VsOut` its full-screen vertex stage
// hands over. A plugin's shader binds nothing itself.
//
// Everything happens in linear light: `source` is an sRGB texture, so the
// sampler has already decoded it, and the target encodes again on store. The
// order — exposure, tint, saturation — is the order `src/grade.rs` documents
// and the golden render test checks against.

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(source, source_sampler, in.uv);

    // 1. Exposure in stops, so +1 doubles the light at every level.
    let exposed = texel.rgb * exp2(params.exposure);

    // 2. The tint is a multiply, mixed in by its amount: it darkens the
    //    channels it is not made of rather than flooding the picture with a
    //    flat colour.
    let tinted = mix(exposed, exposed * params.tint.rgb, vec3<f32>(params.tint_amount));

    // 3. Saturation towards Rec.709 luma, the primaries the MVP composites in.
    let luma = dot(tinted, vec3<f32>(0.2126, 0.7152, 0.0722));
    let graded = mix(vec3<f32>(luma), tinted, vec3<f32>(params.saturation));

    // 4. A floor at zero: a saturation above one can drive a channel negative,
    //    and negative light is not a colour. Alpha is handed on untouched, so
    //    the effect never changes what the compositor blends underneath.
    return vec4<f32>(max(graded, vec3<f32>(0.0)), texel.a);
}
