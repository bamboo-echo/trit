@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> partials: array<f32>;

var<workgroup> scratch: array<f32, 256>;

@compute @workgroup_size(256)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    var sum = 0.0;
    var i = gid.x;
    let stride = 256u * 16384u;
    loop {
        if (i >= arrayLength(&a)) { break; }
        sum = sum + a[i] * b[i];
        i = i + stride;
    }
    scratch[lid] = sum;
    workgroupBarrier();
    var offset = 128u;
    loop {
        if (offset == 0u) { break; }
        if (lid < offset) { scratch[lid] = scratch[lid] + scratch[lid + offset]; }
        workgroupBarrier();
        offset = offset >> 1u;
    }
    if (lid == 0u) { partials[wid.x] = scratch[0]; }
}
