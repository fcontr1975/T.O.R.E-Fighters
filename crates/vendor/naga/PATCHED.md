# naga 27.0.3, patched

This is [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga) 27.0.3 from
crates.io with one fix. `CHANGELOG.md`, `Cargo.toml.orig`, `Cargo.lock` and the
repository dotfiles were left out.

**The fix:** naga declares the SPIR-V `SampleMask` built-in as a plain `u32`.
Vulkan requires an array of 32-bit integers
(`VUID-SampleMask-SampleMask-04359`), and `spirv-val` rejects naga's output. NVIDIA
accepts it. On AMD's Windows driver, the pipeline `City fill surfaces fading in`,
the first to write the mask, crashes `amdvlk64.dll` with a null read (issue #8).

The patch is in `src/back/spv`: the `sample_mask_varyings` field in `mod.rs`, and
`write_varying`, `load_io_with_f16_polyfill`, `store_io_with_f16_polyfill` and
`sample_mask_element` in `writer.rs`. Each place is marked `T.O.R.E patch`.

**Removing it:** naga 30.0.1 still has the bug. When a wgpu update brings a naga
that declares an array, delete this folder and the `exclude` and `[patch.crates-io]`
entries in the root `Cargo.toml`. The test
`crt::shader_tests::the_sample_mask_is_an_array_in_spirv` fails if an update drops the fix
while this folder is still needed.
