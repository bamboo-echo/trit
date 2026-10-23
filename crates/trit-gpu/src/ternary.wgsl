@group(0) @binding(0) var<storage, read> a_pos: array<u32>;
@group(0) @binding(1) var<storage, read> a_neg: array<u32>;
@group(0) @binding(2) var<storage, read> b_pos: array<u32>;
@group(0) @binding(3) var<storage, read> b_neg: array<u32>;
@group(0) @binding(4) var<storage, read_write> out: array<atomic<i32>>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&a_pos)) { return; }
    let pp = i32(countOneBits(a_pos[i] & b_pos[i]));
    let pn = i32(countOneBits(a_pos[i] & b_neg[i]));
    let np = i32(countOneBits(a_neg[i] & b_pos[i]));
    let nn = i32(countOneBits(a_neg[i] & b_neg[i]));
    atomicAdd(&out[0], pp - pn - np + nn);
}
